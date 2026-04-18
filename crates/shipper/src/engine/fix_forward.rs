//! Fix-forward: supersession plan from a compromised receipt (#98 PR 3).
//!
//! When a published release turns out to be compromised (CVE, leaked
//! secret, broken artifact), the remediation options are:
//!
//! 1. **Yank** — containment. Prevents NEW resolves but leaves
//!    existing lockfiles unchanged. Covered by `shipper yank` (PR 1)
//!    and `shipper plan-yank` (PR 2).
//! 2. **Fix-forward** — ship a successor release that replaces the
//!    compromised one. Downstream consumers pick it up on
//!    `cargo update`. **This module plans that.**
//!
//! The two strategies are complementary: an operator typically runs
//! the yank plan *alongside* a fix-forward so new resolves steer away
//! from the bad chain AND existing consumers have something cleaner to
//! upgrade to.
//!
//! ## What this module does
//!
//! - Read a receipt, find the compromised packages (those with
//!   `compromised_at.is_some()`)
//! - Compute a minimal **supersession plan**: each compromised package
//!   needs its successor version to be published, in the same
//!   topological order as the original plan (dependencies first)
//! - Present the plan as either a human-readable step list or JSON
//!
//! ## What this module does NOT do (yet)
//!
//! - **Edit Cargo.toml files** to bump versions. That's workspace-edit
//!   territory — invasive enough to deserve its own PR with dry-run /
//!   --apply / git-guard semantics.
//! - **Run the bumped publish**. Once the operator has bumped versions
//!   and committed them, `shipper publish` handles the actual train
//!   exactly as for any release. Fix-forward's job is the planning
//!   layer, not the execution.
//! - **Chain successor → receipt** via the `superseded_by` field.
//!   Wiring that requires post-publish receipt amendment from the
//!   successor run; another follow-on.
//!
//! Keeping the first PR to *planning only* matches the scope pattern
//! of `plan-yank` (PR 2) — give operators a text blueprint, let them
//! apply it, leave execution orchestration for a later pass once the
//! shape is validated in the field.

use std::path::Path;

use anyhow::{Context, Result};
use shipper_types::{PackageReceipt, PackageState, Receipt};

/// Default successor-version bump strategy. `None` means
/// "operator-supplied suggestion"; the plan just echoes a placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuccessorStrategy {
    /// Print `<OLD>-next` as the suggested version. Lets the operator
    /// pick the actual bump (patch/minor/major) after reading the plan.
    PlaceholderNext,
}

/// One step in a fix-forward plan.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FixForwardStep {
    pub name: String,
    pub current_version: String,
    pub suggested_successor: String,
    /// The `compromised_by` reason copied from the receipt, so an
    /// operator running the plan sees per-crate context without having
    /// to cross-reference the receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A fix-forward plan produced from a receipt.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FixForwardPlan {
    pub plan_id: String,
    pub registry: String,
    /// How many receipt packages were flagged compromised.
    pub compromised_count: usize,
    /// Steps in topological order (dependencies first, dependents last).
    /// The operator applies these by bumping Cargo.toml and running
    /// `shipper publish` for the successor version.
    pub steps: Vec<FixForwardStep>,
}

fn is_compromised(p: &PackageReceipt) -> bool {
    p.compromised_at.is_some() && matches!(p.state, PackageState::Published)
}

fn suggest_next(current: &str, strategy: SuccessorStrategy) -> String {
    match strategy {
        SuccessorStrategy::PlaceholderNext => format!("{current}-next"),
    }
}

/// Build a fix-forward plan from a receipt.
///
/// Topological order — same direction as `publish`, **opposite** of
/// `plan-yank`. Rationale: for fix-forward you're *publishing*
/// replacements, so dependencies go first (downstream fixes can pull
/// updated deps); for yank you're *removing* reachability, so
/// dependents go first.
pub fn build_plan(receipt: &Receipt, strategy: SuccessorStrategy) -> FixForwardPlan {
    let steps: Vec<FixForwardStep> = receipt
        .packages
        .iter()
        .filter(|p| is_compromised(p))
        .map(|p| FixForwardStep {
            name: p.name.clone(),
            current_version: p.version.clone(),
            suggested_successor: suggest_next(&p.version, strategy),
            reason: p.compromised_by.clone(),
        })
        .collect();

    FixForwardPlan {
        plan_id: receipt.plan_id.clone(),
        registry: receipt.registry.name.clone(),
        compromised_count: steps.len(),
        steps,
    }
}

/// Render a fix-forward plan as a human-readable step list. The output
/// is a numbered sequence: bump the Cargo.toml version for each crate,
/// then a single `shipper publish` at the end to ship the lot.
pub fn render_text(plan: &FixForwardPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# fix-forward plan — registry={}, plan_id={}\n",
        plan.registry, plan.plan_id
    ));
    out.push_str(&format!(
        "# {} package(s) marked compromised\n",
        plan.compromised_count
    ));
    if plan.steps.is_empty() {
        out.push_str(
            "# (nothing to fix-forward: no receipt package has compromised_at set. \
             Run `shipper yank --crate <N> --version <V> --reason <R> --mark-compromised` \
             first, or edit receipt.json by hand.)\n",
        );
        return out;
    }
    out.push_str(
        "# Steps:\n\
         #   1. For each crate below, bump the version in its Cargo.toml to the\n\
         #      suggested successor (or your preferred bump).\n\
         #   2. Commit the bumps; they're part of the fix-forward audit trail.\n\
         #   3. Run `shipper publish` to ship the successors in topo order.\n\
         #   4. Once all successors are live, optionally run `shipper plan-yank\n\
         #      --from-receipt <path> --compromised-only` to contain the\n\
         #      compromised versions.\n\
         #\n",
    );
    for (i, step) in plan.steps.iter().enumerate() {
        let reason = step
            .reason
            .as_deref()
            .map(|r| format!("  # {r}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "{:>3}. {}: {} -> {}{reason}\n",
            i + 1,
            step.name,
            step.current_version,
            step.suggested_successor
        ));
    }
    out
}

/// Load a receipt and build a fix-forward plan. Convenience wrapper
/// used by the `shipper fix-forward --from-receipt` CLI path.
pub fn plan_from_path(path: &Path, strategy: SuccessorStrategy) -> Result<FixForwardPlan> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read receipt at {}", path.display()))?;
    let receipt: Receipt = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse receipt at {}", path.display()))?;
    Ok(build_plan(&receipt, strategy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use shipper_types::{
        EnvironmentFingerprint, PackageEvidence, PackageReceipt, PackageState, Receipt, Registry,
    };
    use std::path::PathBuf;

    fn pkg(name: &str, state: PackageState, compromised: Option<&str>) -> PackageReceipt {
        PackageReceipt {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 5,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
            compromised_at: compromised.map(|_| Utc::now()),
            compromised_by: compromised.map(str::to_string),
            superseded_by: None,
        }
    }

    fn sample_receipt(packages: Vec<PackageReceipt>) -> Receipt {
        Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-sample".to_string(),
            registry: Registry::crates_io(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            packages,
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".into(),
                cargo_version: None,
                rust_version: None,
                os: "test".into(),
                arch: "x86_64".into(),
            },
        }
    }

    #[test]
    fn only_compromised_published_packages_produce_steps() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, Some("CVE-2026-0001")),
            pkg(
                "c",
                PackageState::Failed {
                    class: shipper_types::ErrorClass::Permanent,
                    message: "no".into(),
                },
                Some("never-shipped"),
            ),
        ]);
        let plan = build_plan(&r, SuccessorStrategy::PlaceholderNext);
        // b is compromised and published (keeps); a isn't compromised
        // (drops); c is compromised but failed so never shipped (drops).
        assert_eq!(plan.compromised_count, 1);
        assert_eq!(plan.steps[0].name, "b");
        assert_eq!(plan.steps[0].current_version, "0.1.0");
        assert_eq!(plan.steps[0].suggested_successor, "0.1.0-next");
        assert_eq!(plan.steps[0].reason.as_deref(), Some("CVE-2026-0001"));
    }

    #[test]
    fn preserves_topological_order() {
        let r = sample_receipt(vec![
            pkg("lib", PackageState::Published, Some("r")),
            pkg("mid", PackageState::Published, Some("r")),
            pkg("top", PackageState::Published, Some("r")),
        ]);
        let plan = build_plan(&r, SuccessorStrategy::PlaceholderNext);
        let names: Vec<_> = plan.steps.iter().map(|s| s.name.clone()).collect();
        // Same order as receipt.packages (dependencies first, dependents last)
        assert_eq!(names, vec!["lib", "mid", "top"]);
    }

    #[test]
    fn empty_plan_when_nothing_compromised() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, None),
        ]);
        let plan = build_plan(&r, SuccessorStrategy::PlaceholderNext);
        assert_eq!(plan.compromised_count, 0);
        assert!(plan.steps.is_empty());
        let text = render_text(&plan);
        assert!(text.contains("nothing to fix-forward"));
        assert!(
            text.contains("--mark-compromised"),
            "empty-plan render should guide the operator toward the missing step"
        );
    }

    #[test]
    fn text_render_enumerates_steps_with_reason() {
        let r = sample_receipt(vec![
            pkg("core", PackageState::Published, Some("CVE-2026-0001")),
            pkg("app", PackageState::Published, Some("CVE-2026-0001")),
        ]);
        let text = render_text(&build_plan(&r, SuccessorStrategy::PlaceholderNext));
        assert!(text.contains("1. core: 0.1.0 -> 0.1.0-next"));
        assert!(text.contains("2. app: 0.1.0 -> 0.1.0-next"));
        assert!(text.contains("CVE-2026-0001"));
        // Instructional preamble points the operator toward publish +
        // plan-yank so fix-forward isn't mistaken for a complete
        // remediation on its own.
        assert!(text.contains("shipper publish"));
        assert!(text.contains("shipper plan-yank"));
    }
}
