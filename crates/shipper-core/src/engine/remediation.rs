//! Receipt-driven remediation dry-run planning.
//!
//! This module composes the existing containment (`plan_yank`) and
//! fix-forward planning primitives into a durable dry-run artifact. It does
//! not execute `cargo yank`, edit manifests, or publish successors.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use shipper_types::{PackageState, Receipt};

use crate::engine::plan_yank;
use crate::state::execution_state;

pub const REMEDIATION_PLAN_SCHEMA_VERSION: &str = "shipper.remediation_plan.v1";
pub const REDACTED_OPERATOR_REASON: &str = "[OPERATOR_REASON_REDACTED]";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationPlan {
    pub schema_version: String,
    pub source_receipt: String,
    pub plan_id: String,
    pub registry: String,
    pub target: RemediationTarget,
    pub affected_packages: Vec<RemediationAffectedPackage>,
    pub yank_order: Vec<RemediationYankStep>,
    pub fix_forward_suggestions: Vec<RemediationFixForwardStep>,
    pub command_sequence: Vec<RemediationCommand>,
    pub risk_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationTarget {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub version: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationAffectedPackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationYankStep {
    pub name: String,
    pub version: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationFixForwardStep {
    pub name: String,
    pub current_version: String,
    pub suggested_successor: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationCommand {
    pub phase: String,
    pub description: String,
    pub argv: Vec<String>,
}

pub fn build_dry_run_plan(
    receipt: &Receipt,
    dependency_graph: &BTreeMap<String, Vec<String>>,
    source_receipt: &Path,
    target_crate: &str,
    target_version: &str,
    reason: &str,
) -> Result<RemediationPlan> {
    if reason.trim().is_empty() {
        bail!("remediation reason must not be empty");
    }
    let recorded_reason = REDACTED_OPERATOR_REASON.to_string();

    let target = receipt
        .packages
        .iter()
        .find(|p| p.name == target_crate)
        .with_context(|| {
            format!(
                "target crate '{target_crate}' is not in receipt {}; available packages: {}",
                source_receipt.display(),
                receipt
                    .packages
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    if target.version != target_version {
        bail!(
            "target {target_crate}@{target_version} does not match receipt version {}",
            target.version
        );
    }
    if !matches!(target.state, PackageState::Published) {
        bail!(
            "target {target_crate}@{target_version} is not published in the receipt; state was {:?}",
            target.state
        );
    }

    let yank_plan = plan_yank::build_plan_from_starting_crate(
        receipt,
        dependency_graph,
        target_crate,
        Some(recorded_reason.clone()),
    )?;

    let affected_packages = yank_plan
        .entries
        .iter()
        .map(|entry| RemediationAffectedPackage {
            name: entry.name.clone(),
            version: entry.version.clone(),
        })
        .collect();

    let yank_order: Vec<RemediationYankStep> = yank_plan
        .entries
        .iter()
        .map(|entry| RemediationYankStep {
            name: entry.name.clone(),
            version: entry.version.clone(),
            reason: entry
                .reason
                .clone()
                .unwrap_or_else(|| recorded_reason.clone()),
        })
        .collect();

    let fix_forward_suggestions = yank_plan
        .entries
        .iter()
        .rev()
        .map(|entry| RemediationFixForwardStep {
            name: entry.name.clone(),
            current_version: entry.version.clone(),
            suggested_successor: format!("{}-next", entry.version),
            reason: entry
                .reason
                .clone()
                .unwrap_or_else(|| recorded_reason.clone()),
        })
        .collect();

    let command_sequence = yank_order
        .iter()
        .map(|step| RemediationCommand {
            phase: "containment".to_string(),
            description: format!("Yank {}@{} after operator review", step.name, step.version),
            argv: vec![
                "shipper".to_string(),
                "yank".to_string(),
                "--crate".to_string(),
                step.name.clone(),
                "--version".to_string(),
                step.version.clone(),
                "--reason".to_string(),
                "<operator-reason>".to_string(),
            ],
        })
        .collect();

    Ok(RemediationPlan {
        schema_version: REMEDIATION_PLAN_SCHEMA_VERSION.to_string(),
        source_receipt: source_receipt.display().to_string(),
        plan_id: receipt.plan_id.clone(),
        registry: receipt.registry.name.clone(),
        target: RemediationTarget {
            crate_name: target_crate.to_string(),
            version: target_version.to_string(),
            reason: recorded_reason,
        },
        affected_packages,
        yank_order,
        fix_forward_suggestions,
        command_sequence,
        risk_notes: vec![
            "Dry-run only: no yanks, manifest edits, or publish commands were executed."
                .to_string(),
            "Yanking is containment, not undo; existing lockfiles and already-downloaded crate bytes are unaffected."
                .to_string(),
            "Fix-forward successors are placeholders; edit manifests, rerun preflight, then publish deliberately."
                .to_string(),
            "Operator-supplied remediation reason text is omitted from durable artifacts; review the incident source before executing commands."
                .to_string(),
        ],
    })
}

pub fn write_dry_run_artifact(state_dir: &Path, plan: &RemediationPlan) -> Result<PathBuf> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;
    let path = execution_state::remediation_plan_path(state_dir);
    execution_state::atomic_write_json(&path, plan)?;
    Ok(path)
}

pub fn load_plan_from_path(path: &Path) -> Result<RemediationPlan> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open remediation plan {}", path.display()))?;
    let plan: RemediationPlan = serde_json::from_reader(file)
        .with_context(|| format!("failed to parse remediation plan {}", path.display()))?;
    if plan.schema_version != REMEDIATION_PLAN_SCHEMA_VERSION {
        bail!(
            "unsupported remediation plan schema_version '{}'; expected '{}'",
            plan.schema_version,
            REMEDIATION_PLAN_SCHEMA_VERSION
        );
    }
    validate_identifier("registry", &plan.registry)?;
    validate_crate_name("target crate", &plan.target.crate_name)?;
    validate_version("target version", &plan.target.version)?;
    for (idx, step) in plan.yank_order.iter().enumerate() {
        validate_crate_name(&format!("yank_order[{idx}].name"), &step.name)?;
        validate_version(&format!("yank_order[{idx}].version"), &step.version)?;
    }
    Ok(plan)
}

fn validate_identifier(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must not be empty");
    }
    if value.starts_with('-')
        || value
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        bail!("{field} contains unsupported characters");
    }
    Ok(())
}

fn validate_crate_name(field: &str, value: &str) -> Result<()> {
    validate_identifier(field, value)
}

fn validate_version(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must not be empty");
    }
    if value.starts_with('-')
        || value
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+'))
    {
        bail!("{field} contains unsupported characters");
    }
    Ok(())
}

pub fn render_text(plan: &RemediationPlan, artifact_path: &Path) -> String {
    let mut out = String::new();
    out.push_str("Remediation dry-run plan\n");
    out.push_str(&format!("artifact: {}\n", artifact_path.display()));
    out.push_str(&format!(
        "target: {}@{}\n",
        plan.target.crate_name, plan.target.version
    ));
    out.push_str(&format!("reason: {}\n", plan.target.reason));
    out.push_str("\nYank order:\n");
    for (idx, step) in plan.yank_order.iter().enumerate() {
        out.push_str(&format!(
            "  {}. shipper yank --crate {} --version {} --reason <recorded>\n",
            idx + 1,
            step.name,
            step.version
        ));
    }
    out.push_str("\nFix-forward suggestions:\n");
    for (idx, step) in plan.fix_forward_suggestions.iter().enumerate() {
        out.push_str(&format!(
            "  {}. {}: {} -> {}\n",
            idx + 1,
            step.name,
            step.current_version,
            step.suggested_successor
        ));
    }
    out.push_str("\nNo yanks, manifest edits, or publishes were executed.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use shipper_types::{
        EnvironmentFingerprint, PackageEvidence, PackageReceipt, PackageState, Receipt, Registry,
    };

    fn pkg(name: &str, version: &str, state: PackageState) -> PackageReceipt {
        PackageReceipt {
            name: name.to_string(),
            version: version.to_string(),
            attempts: 1,
            state,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 1,
            evidence: PackageEvidence {
                attempts: Vec::new(),
                readiness_checks: Vec::new(),
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        }
    }

    fn receipt(packages: Vec<PackageReceipt>) -> Receipt {
        Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-remediation".to_string(),
            registry: Registry::crates_io(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            packages,
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "test".to_string(),
                cargo_version: None,
                rust_version: None,
                os: "test".to_string(),
                arch: "test".to_string(),
            },
            auth_evidence: None,
        }
    }

    fn dependency_graph() -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([
            ("core-lib".to_string(), Vec::new()),
            ("mid-lib".to_string(), vec!["core-lib".to_string()]),
            ("top-app".to_string(), vec!["mid-lib".to_string()]),
        ])
    }

    #[test]
    fn dry_run_plan_names_target_and_orders_containment_and_fix_forward() {
        let receipt = receipt(vec![
            pkg("core-lib", "0.2.0", PackageState::Published),
            pkg("mid-lib", "0.3.0", PackageState::Published),
            pkg("top-app", "0.4.0", PackageState::Published),
        ]);

        let plan = build_dry_run_plan(
            &receipt,
            &dependency_graph(),
            Path::new(".shipper/receipt.json"),
            "core-lib",
            "0.2.0",
            "CVE-2026-0001",
        )
        .expect("plan");

        assert_eq!(plan.schema_version, REMEDIATION_PLAN_SCHEMA_VERSION);
        assert_eq!(plan.target.crate_name, "core-lib");
        assert_eq!(plan.affected_packages.len(), 3);
        let yank_names: Vec<_> = plan.yank_order.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(yank_names, vec!["top-app", "mid-lib", "core-lib"]);
        let fix_names: Vec<_> = plan
            .fix_forward_suggestions
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(fix_names, vec!["core-lib", "mid-lib", "top-app"]);
        assert_eq!(plan.command_sequence.len(), 3);
        assert_eq!(plan.command_sequence[0].argv[0], "shipper");
    }

    #[test]
    fn dry_run_plan_omits_arbitrary_operator_reason() {
        let receipt = receipt(vec![pkg("core-lib", "0.2.0", PackageState::Published)]);

        let sensitive_reason = "plain-text incident token secret123";
        let plan = build_dry_run_plan(
            &receipt,
            &dependency_graph(),
            Path::new(".shipper/receipt.json"),
            "core-lib",
            "0.2.0",
            sensitive_reason,
        )
        .expect("plan");

        let raw = serde_json::to_string(&plan).expect("serialize");
        assert!(!raw.contains(sensitive_reason));
        assert!(!raw.contains("secret123"));
        assert!(raw.contains(REDACTED_OPERATOR_REASON));
        assert!(raw.contains("<operator-reason>"));
    }

    #[test]
    fn dry_run_plan_rejects_target_version_mismatch() {
        let receipt = receipt(vec![pkg("core-lib", "0.2.0", PackageState::Published)]);

        let err = build_dry_run_plan(
            &receipt,
            &dependency_graph(),
            Path::new(".shipper/receipt.json"),
            "core-lib",
            "9.9.9",
            "bad release",
        )
        .expect_err("version mismatch");

        assert!(err.to_string().contains("does not match receipt version"));
    }
}
