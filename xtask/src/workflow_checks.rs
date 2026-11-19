//! Workflow / process / network policy checks.
//!
//! Three subcommands:
//!
//! - `cargo xtask check-workflow-surfaces`  — every `.github/workflows/*.yml`
//!   (and `.github/dependabot.yml`) must be receipted in
//!   `policy/workflow-allowlist.toml`. Each entry must name a
//!   `process_policy` and `network_policy` that exist in their respective
//!   ledgers.
//! - `cargo xtask check-process-policy`     — for each receipted workflow,
//!   scan its file content for command names; flag commands present in any
//!   other process profile but NOT in this workflow's declared profile.
//! - `cargo xtask check-network-policy`     — for each receipted workflow,
//!   scan its file content for `https?://<host>` URLs; flag hostnames not in
//!   the declared network profile.
//!
//! All three accept `--mode advisory|blocking-allowlist|blocking-strict`.
//! The user's spec for PR 8 says explicitly "start simple": these checks are
//! grep-style heuristics, not full YAML/AST parsers. Advisory mode is the
//! default and what CI runs (PR 10).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::authority_exceptions::{self, AuthorityException};

const OUTPUT_DIR_REL: &str = "target/policy";

const WORKFLOW_ALLOWLIST: &str = "policy/workflow-allowlist.toml";
const PROCESS_ALLOWLIST: &str = "policy/process-allowlist.toml";
const NETWORK_ALLOWLIST: &str = "policy/network-allowlist.toml";

/// Shared CLI mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    Advisory,
    BlockingAllowlist,
    BlockingStrict,
}

// ─── Allowlist deserialization ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WorkflowAllowlistDoc {
    #[serde(default)]
    workflow: Vec<RawWorkflowEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawWorkflowEntry {
    path: Option<String>,
    kind: Option<String>,
    owner: Option<String>,
    reason: Option<String>,
    process_policy: Option<String>,
    network_policy: Option<String>,
    required_repository_guard: Option<String>,
    created: Option<String>,
    review_after: Option<String>,
    expires: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileDoc {
    #[serde(default)]
    profile: Vec<RawProfile>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProfile {
    name: Option<String>,
    #[serde(default)]
    allowed_processes: Vec<String>,
    #[serde(default)]
    allowed_endpoints: Vec<String>,
    owner: Option<String>,
    reason: Option<String>,
    created: Option<String>,
    review_after: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileKind {
    Process,
    Network,
}

impl ProfileKind {
    fn reference(self, entry: &RawWorkflowEntry) -> Option<&str> {
        match self {
            Self::Process => entry.process_policy.as_deref(),
            Self::Network => entry.network_policy.as_deref(),
        }
    }
}

#[derive(Debug, Clone)]
struct ProfileCatalog {
    entries: usize,
    profiles: BTreeMap<String, RawProfile>,
    findings: ProfileLifecycleFindings,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ProfileLifecycleFindings {
    missing_fields: Vec<ProfileMissingFields>,
    invalid_profiles: Vec<InvalidProfile>,
    stale: Vec<StaleProfile>,
    orphaned: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProfileMissingFields {
    profile: String,
    missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct InvalidProfile {
    profile: String,
    field: &'static str,
    value: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StaleProfile {
    profile: String,
    review_after: String,
    today: String,
}

// ─── check-workflow-surfaces ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct WorkflowReport {
    tool: &'static str,
    mode: &'static str,
    today: String,
    summary: WorkflowSummary,
    findings: WorkflowFindings,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowSummary {
    tracked_workflow_files: usize,
    allowlist_entries: usize,
    unreceipted: usize,
    missing_fields: usize,
    expired: usize,
    stale: usize,
    unused: usize,
    invalid_policy_refs: usize,
    repository_guard_violations: usize,
    /// Raw detector total. The exception buckets below partition it, so an
    /// accepted capability stays visible instead of disappearing from the
    /// report the moment someone writes a ledger record for it.
    authority_violations: usize,
    authorized_exceptions: usize,
    unexcepted_authority: usize,
    expired_exceptions: usize,
    drifted_exceptions: usize,
    unused_exceptions: usize,
    invalid_authority_ledger: usize,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowFindings {
    unreceipted: Vec<String>,
    missing_fields: Vec<MissingFields>,
    expired: Vec<ExpiredEntry>,
    stale: Vec<StaleEntry>,
    unused: Vec<String>,
    invalid_policy_refs: Vec<InvalidPolicyRef>,
    repository_guard_violations: Vec<RepositoryGuardViolation>,
    authority_violations: Vec<WorkflowAuthorityFinding>,
    authorized_exceptions: Vec<AuthorizedException>,
    unexcepted_authority: Vec<WorkflowAuthorityFinding>,
    expired_exceptions: Vec<ExpiredException>,
    drifted_exceptions: Vec<DriftedException>,
    unused_exceptions: Vec<UnusedException>,
    invalid_authority_ledger: Vec<String>,
}

/// A workflow capability that needs an explicit authority decision.
///
/// 257A deliberately reports these findings before 257B makes the exception
/// ledger blocking. Keeping the finding structured here lets the enforcement
/// PR consume the same evidence without replacing the detector with a second
/// parser.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct WorkflowAuthorityFinding {
    workflow: String,
    job: String,
    step: String,
    capability: String,
    trigger: String,
    repository_boundary: String,
    remediation: String,
}

#[derive(Debug, Clone, Serialize)]
struct MissingFields {
    entry: String,
    missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ExpiredEntry {
    entry: String,
    expires: String,
    today: String,
}

#[derive(Debug, Clone, Serialize)]
struct StaleEntry {
    entry: String,
    review_after: String,
    today: String,
}

#[derive(Debug, Clone, Serialize)]
struct InvalidPolicyRef {
    workflow: String,
    policy_kind: &'static str, // "process_policy" | "network_policy"
    named: String,
    available: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RepositoryGuardViolation {
    workflow: String,
    required_repository: String,
    job: String,
    reason: String,
}

pub fn check_workflow_surfaces(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;
    let workflows = tracked_workflow_files(&workspace_root)?;
    let all_entries = load_workflow_allowlist(&workspace_root)?;
    // `dependabot_config` entries live in workflow-allowlist for catalog
    // purposes but are not workflow files — skip them from the workflow-
    // surface reconciliation. They still get receipt validation (missing
    // fields, expired, stale) via their own loop below.
    let entries: Vec<RawWorkflowEntry> = all_entries
        .iter()
        .filter(|e| !is_dependabot_config(e))
        .cloned()
        .collect();
    let dependabot_entries: Vec<RawWorkflowEntry> = all_entries
        .iter()
        .filter(|e| is_dependabot_config(e))
        .cloned()
        .collect();
    let process_profiles = load_profile_names(&workspace_root, PROCESS_ALLOWLIST)?;
    let network_profiles = load_profile_names(&workspace_root, NETWORK_ALLOWLIST)?;
    let today = today_iso();

    // unreceipted / unused
    let entry_paths: BTreeSet<String> = entries.iter().filter_map(|e| e.path.clone()).collect();
    let workflow_set: BTreeSet<&str> = workflows.iter().map(String::as_str).collect();

    let unreceipted: Vec<String> = workflows
        .iter()
        .filter(|p| !entry_paths.contains(p.as_str()))
        .cloned()
        .collect();
    let unused: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            e.path
                .as_ref()
                .filter(|p| !workflow_set.contains(p.as_str()))
                .cloned()
        })
        .collect();

    // missing_fields, expired, stale — across ALL entries (including
    // dependabot_config catalog entries) so their receipts get validated too.
    let missing_fields: Vec<MissingFields> = all_entries
        .iter()
        .filter_map(|e| {
            let missing = missing_workflow_fields(e);
            if missing.is_empty() {
                None
            } else {
                Some(MissingFields {
                    entry: format!("workflow: {}", e.path.clone().unwrap_or_default()),
                    missing,
                })
            }
        })
        .collect();

    let expired: Vec<ExpiredEntry> = all_entries
        .iter()
        .filter_map(|e| {
            e.expires.as_ref().and_then(|exp| {
                if date_is_past(exp, &today) {
                    Some(ExpiredEntry {
                        entry: format!("workflow: {}", e.path.clone().unwrap_or_default()),
                        expires: exp.clone(),
                        today: today.clone(),
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    let stale: Vec<StaleEntry> = all_entries
        .iter()
        .filter_map(|e| {
            e.review_after.as_ref().and_then(|rev| {
                if date_is_past(rev, &today) {
                    Some(StaleEntry {
                        entry: format!("workflow: {}", e.path.clone().unwrap_or_default()),
                        review_after: rev.clone(),
                        today: today.clone(),
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    // invalid policy refs — checked across ALL entries; even
    // dependabot_config entries name policies.
    let mut invalid_policy_refs: Vec<InvalidPolicyRef> = Vec::new();
    for e in &all_entries {
        let label = e.path.clone().unwrap_or_default();
        if let Some(named) = &e.process_policy
            && !process_profiles.contains(named)
        {
            invalid_policy_refs.push(InvalidPolicyRef {
                workflow: label.clone(),
                policy_kind: "process_policy",
                named: named.clone(),
                available: process_profiles.iter().cloned().collect(),
            });
        }
        if let Some(named) = &e.network_policy
            && !network_profiles.contains(named)
        {
            invalid_policy_refs.push(InvalidPolicyRef {
                workflow: label.clone(),
                policy_kind: "network_policy",
                named: named.clone(),
                available: network_profiles.iter().cloned().collect(),
            });
        }
    }

    let repository_guard_violations = repository_guard_violations(&workspace_root, &entries);
    let authority_violations = workflow_authority_violations(&workspace_root, &entries);

    // Reconcile every authority finding against the exact exception ledger.
    // `NaiveDate::MAX` as the fallback fails closed: with no readable date,
    // every record reads as expired rather than as a live authorization.
    let today_date = NaiveDate::parse_from_str(&today, "%Y-%m-%d").ok();
    let (authority_records, invalid_authority_ledger) =
        load_authority_records(&workspace_root, today_date);
    let reconciliation = reconcile_authority_exceptions(
        &authority_violations,
        &authority_records,
        today_date.unwrap_or(NaiveDate::MAX),
    );

    let findings = WorkflowFindings {
        unreceipted,
        missing_fields,
        expired,
        stale,
        unused,
        invalid_policy_refs,
        repository_guard_violations,
        authority_violations,
        authorized_exceptions: reconciliation.authorized,
        unexcepted_authority: reconciliation.unexcepted,
        expired_exceptions: reconciliation.expired,
        drifted_exceptions: reconciliation.drifted,
        unused_exceptions: reconciliation.unused,
        invalid_authority_ledger,
    };

    let _ = dependabot_entries; // tracked-but-skipped; kept for future per-kind audits.

    let summary = WorkflowSummary {
        tracked_workflow_files: workflows.len(),
        allowlist_entries: all_entries.len(),
        unreceipted: findings.unreceipted.len(),
        missing_fields: findings.missing_fields.len(),
        expired: findings.expired.len(),
        stale: findings.stale.len(),
        unused: findings.unused.len(),
        invalid_policy_refs: findings.invalid_policy_refs.len(),
        repository_guard_violations: findings.repository_guard_violations.len(),
        authority_violations: findings.authority_violations.len(),
        authorized_exceptions: findings.authorized_exceptions.len(),
        unexcepted_authority: findings.unexcepted_authority.len(),
        expired_exceptions: findings.expired_exceptions.len(),
        drifted_exceptions: findings.drifted_exceptions.len(),
        unused_exceptions: findings.unused_exceptions.len(),
        invalid_authority_ledger: findings.invalid_authority_ledger.len(),
    };

    let report = WorkflowReport {
        tool: "cargo xtask check-workflow-surfaces",
        mode: mode_str(mode),
        today,
        summary,
        findings,
    };

    write_workflow_report(&workspace_root, &report)?;
    println!(
        "{} ({}): workflows={} entries={} unreceipted={} missing_fields={} expired={} stale={} unused={} invalid_refs={} repository_guard_violations={} authority_violations={} authorized_exceptions={} unexcepted_authority={} expired_exceptions={} drifted_exceptions={} unused_exceptions={} invalid_authority_ledger={}",
        report.tool,
        report.mode,
        report.summary.tracked_workflow_files,
        report.summary.allowlist_entries,
        report.summary.unreceipted,
        report.summary.missing_fields,
        report.summary.expired,
        report.summary.stale,
        report.summary.unused,
        report.summary.invalid_policy_refs,
        report.summary.repository_guard_violations,
        report.summary.authority_violations,
        report.summary.authorized_exceptions,
        report.summary.unexcepted_authority,
        report.summary.expired_exceptions,
        report.summary.drifted_exceptions,
        report.summary.unused_exceptions,
        report.summary.invalid_authority_ledger,
    );

    let blocking = workflow_blocking_count(mode, &report.findings);
    if blocking > 0 && !matches!(mode, Mode::Advisory) {
        bail!(
            "{}: {} mode found {} blocking issue(s); see {}/workflow-policy-report.md",
            report.tool,
            report.mode,
            blocking,
            OUTPUT_DIR_REL
        );
    }
    Ok(())
}

fn missing_workflow_fields(e: &RawWorkflowEntry) -> Vec<String> {
    let mut missing = Vec::new();
    if e.path.is_none() {
        missing.push("path".to_string());
    }
    for (name, present) in [
        ("kind", e.kind.is_some()),
        ("owner", e.owner.is_some()),
        ("reason", e.reason.is_some()),
        ("process_policy", e.process_policy.is_some()),
        ("network_policy", e.network_policy.is_some()),
        ("created", e.created.is_some()),
        ("review_after", e.review_after.is_some()),
    ] {
        if !present {
            missing.push(name.to_string());
        }
    }
    missing
}

fn workflow_blocking_count(mode: Mode, f: &WorkflowFindings) -> usize {
    // Every authority state except a live, exactly-matched exception blocks.
    // `authorized_exceptions` is deliberately absent: it is the one accepted
    // state, and it stays visible in the reports rather than in this count.
    let mut n = f.unreceipted.len()
        + f.missing_fields.len()
        + f.expired.len()
        + f.invalid_policy_refs.len()
        + f.repository_guard_violations.len()
        + f.unexcepted_authority.len()
        + f.expired_exceptions.len()
        + f.drifted_exceptions.len()
        + f.unused_exceptions.len()
        + f.invalid_authority_ledger.len();
    if matches!(mode, Mode::BlockingStrict) {
        n += f.unused.len() + f.stale.len();
    }
    n
}

fn write_workflow_report(workspace_root: &Path, r: &WorkflowReport) -> Result<()> {
    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json = serde_json::to_string_pretty(r).context("serializing workflow report")?;
    fs::write(out_dir.join("workflow-policy-report.json"), json)
        .context("writing workflow-policy-report.json")?;
    let md = render_workflow_md(r);
    fs::write(out_dir.join("workflow-policy-report.md"), md)
        .context("writing workflow-policy-report.md")?;
    Ok(())
}

fn render_workflow_md(r: &WorkflowReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} Report\n\n", r.tool));
    out.push_str(&format!(
        "Generated by `{} --mode {}` on {}.\n\n",
        r.tool, r.mode, r.today
    ));
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Tracked workflow files: {}\n",
        r.summary.tracked_workflow_files
    ));
    out.push_str(&format!(
        "- Allowlist entries: {}\n",
        r.summary.allowlist_entries
    ));
    out.push_str(&format!("- Unreceipted: {}\n", r.summary.unreceipted));
    out.push_str(&format!("- Missing fields: {}\n", r.summary.missing_fields));
    out.push_str(&format!("- Expired: {}\n", r.summary.expired));
    out.push_str(&format!("- Stale review: {}\n", r.summary.stale));
    out.push_str(&format!("- Unused: {}\n", r.summary.unused));
    out.push_str(&format!(
        "- Invalid policy refs: {}\n\n",
        r.summary.invalid_policy_refs
    ));
    out.push_str(&format!(
        "- Repository guard violations: {}\n\n",
        r.summary.repository_guard_violations
    ));
    out.push_str(&format!(
        "- Authority violations: {}\n",
        r.summary.authority_violations
    ));
    out.push_str(&format!(
        "- Authorized exceptions (accepted): {}\n",
        r.summary.authorized_exceptions
    ));
    out.push_str(&format!(
        "- Unexcepted authority: {}\n",
        r.summary.unexcepted_authority
    ));
    out.push_str(&format!(
        "- Expired exceptions: {}\n",
        r.summary.expired_exceptions
    ));
    out.push_str(&format!(
        "- Drifted exceptions: {}\n",
        r.summary.drifted_exceptions
    ));
    out.push_str(&format!(
        "- Unused exceptions: {}\n",
        r.summary.unused_exceptions
    ));
    out.push_str(&format!(
        "- Invalid authority ledger: {}\n\n",
        r.summary.invalid_authority_ledger
    ));
    list_strings(&mut out, "Unreceipted workflows", &r.findings.unreceipted);
    for m in &r.findings.missing_fields {
        out.push_str(&format!(
            "- `{}`: missing {}\n",
            m.entry,
            m.missing.join(", ")
        ));
    }
    for ipr in &r.findings.invalid_policy_refs {
        out.push_str(&format!(
            "- INVALID {}: `{}` references `{}` which is not in {{{}}}\n",
            ipr.policy_kind,
            ipr.workflow,
            ipr.named,
            ipr.available.join(", ")
        ));
    }
    for guard in &r.findings.repository_guard_violations {
        out.push_str(&format!(
            "- REPOSITORY GUARD: `{}` job `{}` must be guarded to `{}` ({})\n",
            guard.workflow, guard.job, guard.required_repository, guard.reason
        ));
    }
    for finding in &r.findings.authority_violations {
        out.push_str(&format!(
            "- AUTHORITY: `{}` job `{}` step `{}` capability `{}` trigger `{}` boundary `{}`; {}\n",
            finding.workflow,
            finding.job,
            finding.step,
            finding.capability,
            finding.trigger,
            finding.repository_boundary,
            finding.remediation,
        ));
    }
    render_authority_exception_sections(&mut out, r);
    out
}

fn render_authority_exception_sections(out: &mut String, r: &WorkflowReport) {
    out.push_str(&format!(
        "\n## Authorized authority exceptions ({})\n\n",
        r.findings.authorized_exceptions.len()
    ));
    if r.findings.authorized_exceptions.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for accepted in &r.findings.authorized_exceptions {
            out.push_str(&format!(
                "- ACCEPTED: `{}` job `{}` step `{}` capability `{}` trigger `{}` boundary `{}`; owned by `{}` in `{}`, review after {}\n",
                accepted.workflow,
                accepted.job,
                accepted.step,
                accepted.capability,
                accepted.trigger,
                accepted.repository_boundary,
                accepted.owner,
                accepted.repository,
                accepted.review_after,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "## Unexcepted authority ({})\n\n",
        r.findings.unexcepted_authority.len()
    ));
    if r.findings.unexcepted_authority.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for finding in &r.findings.unexcepted_authority {
            out.push_str(&format!(
                "- UNEXCEPTED: `{}` job `{}` step `{}` capability `{}` trigger `{}` boundary `{}`; {}\n",
                finding.workflow,
                finding.job,
                finding.step,
                finding.capability,
                finding.trigger,
                finding.repository_boundary,
                finding.remediation,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "## Expired authority exceptions ({})\n\n",
        r.findings.expired_exceptions.len()
    ));
    if r.findings.expired_exceptions.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for expired in &r.findings.expired_exceptions {
            out.push_str(&format!(
                "- EXPIRED: `{}` job `{}` step `{}` capability `{}`; owned by `{}`, review_after {} is before {}\n",
                expired.workflow,
                expired.job,
                expired.step,
                expired.capability,
                expired.owner,
                expired.review_after,
                expired.today,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "## Drifted authority exceptions ({})\n\n",
        r.findings.drifted_exceptions.len()
    ));
    if r.findings.drifted_exceptions.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for drifted in &r.findings.drifted_exceptions {
            let fields = drifted
                .drifted
                .iter()
                .map(|field| {
                    format!(
                        "{} (recorded `{}`, detected `{}`)",
                        field.field, field.expected, field.actual
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            out.push_str(&format!(
                "- DRIFTED: `{}` job `{}` step `{}` capability `{}` owned by `{}` authorizes nothing — {}; {}\n",
                drifted.workflow,
                drifted.job,
                drifted.step,
                drifted.capability,
                drifted.owner,
                fields,
                drifted.remediation,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "## Unused authority exceptions ({})\n\n",
        r.findings.unused_exceptions.len()
    ));
    if r.findings.unused_exceptions.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for unused in &r.findings.unused_exceptions {
            out.push_str(&format!(
                "- UNUSED: `{}` job `{}` step `{}` capability `{}` trigger `{}` boundary `{}` owned by `{}` (review after {}); {}\n",
                unused.workflow,
                unused.job,
                unused.step,
                unused.capability,
                unused.trigger,
                unused.finding_repository_boundary,
                unused.owner,
                unused.review_after,
                unused.reason,
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "## Invalid authority ledger ({})\n\n",
        r.findings.invalid_authority_ledger.len()
    ));
    if r.findings.invalid_authority_ledger.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for error in &r.findings.invalid_authority_ledger {
            out.push_str(&format!("- INVALID LEDGER: {error}\n"));
        }
        out.push('\n');
    }
}

fn repository_guard_violations(
    workspace_root: &Path,
    entries: &[RawWorkflowEntry],
) -> Vec<RepositoryGuardViolation> {
    let mut violations = Vec::new();

    for entry in entries {
        let path = entry.path.clone().unwrap_or_default();
        let required_repository = match entry.required_repository_guard.as_ref() {
            Some(repo) => repo.clone(),
            None if entry.kind.as_deref() == Some("release") => {
                violations.push(RepositoryGuardViolation {
                    workflow: path,
                    required_repository: "EffortlessMetrics/shipper".to_string(),
                    job: "<allowlist>".to_string(),
                    reason: "release workflow is missing required_repository_guard in workflow allowlist"
                        .to_string(),
                });
                continue;
            }
            None => continue,
        };

        let content = read_workflow_content(workspace_root, &path).unwrap_or_default();
        let unguarded_jobs = workflow_jobs_missing_repository_guard(&content, &required_repository);
        if unguarded_jobs.is_empty() {
            continue;
        }
        for job in unguarded_jobs {
            violations.push(RepositoryGuardViolation {
                workflow: path.clone(),
                required_repository: required_repository.clone(),
                job,
                reason:
                    "job-level if does not contain the required github.repository equality guard"
                        .to_string(),
            });
        }
    }

    violations
}

#[derive(Debug, Clone)]
struct WorkflowStepBlock {
    name: String,
    content: String,
}

#[derive(Debug, Clone)]
struct PermissionScope {
    job: String,
    values: BTreeMap<String, String>,
}

/// Detect workflow authority hazards without pretending to be a general YAML
/// parser. The workflow checker intentionally works on the same small,
/// indentation-aware surface as the existing repository-guard checker. YAML
/// syntax validation remains actionlint's responsibility; this function owns
/// only authority-bearing patterns that need a stable policy signal.
fn workflow_authority_violations(
    workspace_root: &Path,
    entries: &[RawWorkflowEntry],
) -> Vec<WorkflowAuthorityFinding> {
    let mut findings = Vec::new();

    for entry in entries {
        if is_dependabot_config(entry) {
            continue;
        }
        let Some(path) = entry.path.as_deref() else {
            continue;
        };
        let Ok(content) = read_workflow_content(workspace_root, path) else {
            continue;
        };
        findings.extend(analyze_workflow_authority(
            path,
            entry.kind.as_deref().unwrap_or("unknown"),
            entry.required_repository_guard.as_deref(),
            &content,
        ));
    }

    findings
}

fn analyze_workflow_authority(
    workflow: &str,
    kind: &str,
    required_repository: Option<&str>,
    yaml_text: &str,
) -> Vec<WorkflowAuthorityFinding> {
    let triggers = workflow_trigger_names(yaml_text);
    let trigger = if triggers.is_empty() {
        "<unknown>".to_string()
    } else {
        triggers.iter().cloned().collect::<Vec<_>>().join(",")
    };
    let workflow_name = workflow_name(yaml_text).unwrap_or_else(|| workflow.to_string());
    let mut findings = Vec::new();

    if temporary_workflow_identity(workflow, &workflow_name) {
        findings.push(authority_finding(
            workflow,
            "<workflow>",
            "<workflow>",
            "temporary-workflow-identity",
            &trigger,
            "repository workflow inventory",
            "rename the durable workflow or add a narrow, owned exception with a review date",
        ));
    }

    for branch in temporary_branch_filters(yaml_text) {
        findings.push(authority_finding(
            workflow,
            "<workflow>",
            "<trigger>",
            &format!("temporary-branch-filter:{branch}"),
            &trigger,
            "repository branch trigger",
            "use a durable documented branch or record an exact bounded maintenance exception",
        ));
    }

    if triggers.contains("labeled") && has_true_cancel_in_progress(yaml_text) {
        findings.push(authority_finding(
            workflow,
            "<workflow>",
            "<concurrency>",
            "label-triggered-cancellation",
            &trigger,
            "meaningful code-change run",
            "separate label-gated work from the meaningful code-change concurrency group or disable cancellation",
        ));
    }

    let job_blocks = workflow_job_blocks(yaml_text);
    let permission_scopes = workflow_permission_scopes(yaml_text, &job_blocks);
    for scope in &permission_scopes {
        for (capability, value) in &scope.values {
            let unknown_scalar = value.strip_prefix("unknown:");
            if value != "write" && value != "*" && unknown_scalar.is_none() {
                continue;
            }
            let reported_capability = unknown_scalar.map_or_else(
                || format!("{capability}:{value}"),
                |scalar| format!("unknown-permission-scalar:{scalar}"),
            );
            let high_risk = matches!(
                capability.as_str(),
                "contents" | "id-token" | "actions" | "workflows"
            );
            let workflow_level = scope.job == "<workflow>";
            let job_guarded = !workflow_level
                && required_repository.is_some_and(|repository| {
                    job_blocks
                        .iter()
                        .find(|(job, _)| job == &scope.job)
                        .is_some_and(|(_, block)| block_has_repository_guard(block, repository))
                });
            if unknown_scalar.is_some()
                || workflow_level
                || capability == "*"
                || (high_risk && (kind != "release" || !job_guarded))
            {
                findings.push(authority_finding(
                    workflow,
                    &scope.job,
                    "<permissions>",
                    &reported_capability,
                    &trigger,
                    required_repository.unwrap_or("not declared"),
                    "move the grant to the narrow job that needs it and prove the repository boundary",
                ));
            }
        }
    }

    for (job, block) in &job_blocks {
        let guarded =
            required_repository.is_some_and(|repo| block_has_repository_guard(block, repo));
        let scopes = workflow_permission_scopes_for_job(block, job);
        let all_scopes = permission_scopes
            .iter()
            .filter(|scope| scope.job == "<workflow>")
            .cloned()
            .chain(scopes.iter().cloned())
            .collect::<Vec<_>>();
        let steps = workflow_step_blocks(block);
        let release_sensitive = kind == "release"
            && (contains_release_authority(block)
                || scopes
                    .iter()
                    .any(|scope| scope.values.get("id-token").is_some_and(|v| v == "write")));

        for step in &steps {
            for capability in self_mutation_capabilities(&step.content) {
                findings.push(authority_finding(
                    workflow,
                    job,
                    &step.name,
                    capability,
                    &trigger,
                    required_repository.unwrap_or("not declared"),
                    "edit through a normal reviewed branch/PR; do not self-commit, self-push, or self-delete workflow source",
                ));
            }
        }

        if release_sensitive && !guarded {
            findings.push(authority_finding(
                workflow,
                job,
                "<job>",
                "release-authority-without-repository-guard",
                &trigger,
                required_repository.unwrap_or("EffortlessMetrics/shipper"),
                "guard every release-sensitive job with the release-authority repository equality",
            ));
        }

        if triggers.contains("pull_request_target")
            && block_has_untrusted_checkout(block)
            && (block.contains("secrets.") || scope_has_write_permission(&all_scopes))
        {
            findings.push(authority_finding(
                workflow,
                job,
                "<checkout>",
                "pull-request-target-untrusted-execution",
                &trigger,
                "untrusted pull request code",
                "do not execute PR-controlled checkout with secrets or write authority; use pull_request with read-only permissions",
            ));
        }
    }

    if kind == "release"
        && triggers.contains("workflow_dispatch")
        && has_dispatch_ref_input(yaml_text)
        && !has_exact_release_identity_language(yaml_text)
    {
        findings.push(authority_finding(
            workflow,
            "<workflow>",
            "<workflow_dispatch.ref>",
            "mutable-release-dispatch-ref",
            &trigger,
            required_repository.unwrap_or("EffortlessMetrics/shipper"),
            "dispatch release work with an approved immutable SHA/tree, never a mutable branch name",
        ));
    }

    if kind == "release"
        && triggers.contains("push")
        && workflow_has_tag_trigger(yaml_text)
        && workflow_contains_release_authority(yaml_text)
        && !has_exact_release_identity_language(yaml_text)
    {
        findings.push(authority_finding(
            workflow,
            "<workflow>",
            "<tag-trigger>",
            "tag-release-without-approved-source-gate",
            &trigger,
            required_repository.unwrap_or("EffortlessMetrics/shipper"),
            "validate approved source SHA/tree, version, package graph, notes, and gates before mutation",
        ));
    }

    findings
}

fn authority_finding(
    workflow: &str,
    job: &str,
    step: &str,
    capability: &str,
    trigger: &str,
    repository_boundary: &str,
    remediation: &str,
) -> WorkflowAuthorityFinding {
    WorkflowAuthorityFinding {
        workflow: workflow.to_string(),
        job: job.to_string(),
        step: step.to_string(),
        capability: capability.to_string(),
        trigger: trigger.to_string(),
        repository_boundary: repository_boundary.to_string(),
        remediation: remediation.to_string(),
    }
}

// ─── Authority exception reconciliation ─────────────────────────────────────

/// Capability prefix the detector emits when a `permissions:` scalar does not
/// resolve into known capability pairs.
///
/// An unparsed authority shape is exactly the case a ledger record must never
/// be able to silence, so records naming this prefix are rejected outright.
const UNKNOWN_PERMISSION_PREFIX: &str = "unknown-permission-scalar:";

/// A detector finding covered by exactly one valid, unexpired ledger record.
///
/// Accepted, not hidden: this stays in both reports so the accepted capability
/// remains visible with its owner and review date.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AuthorizedException {
    workflow: String,
    job: String,
    step: String,
    capability: String,
    trigger: String,
    repository_boundary: String,
    repository: String,
    owner: String,
    review_after: String,
}

/// A detector finding whose matching record is past its review date.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ExpiredException {
    workflow: String,
    job: String,
    step: String,
    capability: String,
    trigger: String,
    repository_boundary: String,
    owner: String,
    review_after: String,
    today: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DriftedField {
    field: &'static str,
    /// The value recorded in the ledger.
    expected: String,
    /// The value the detector actually observed.
    actual: String,
}

/// A record that names the same workflow/job/step/capability as a finding but
/// authorizes a different trigger or repository boundary.
///
/// A drifted record authorizes nothing: the authority it was written for is no
/// longer the authority in the tree.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DriftedException {
    workflow: String,
    job: String,
    step: String,
    capability: String,
    owner: String,
    drifted: Vec<DriftedField>,
    remediation: String,
}

/// A ledger record that authorized no detector finding.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct UnusedException {
    workflow: String,
    job: String,
    step: String,
    capability: String,
    trigger: String,
    finding_repository_boundary: String,
    owner: String,
    review_after: String,
    reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AuthorityReconciliation {
    authorized: Vec<AuthorizedException>,
    unexcepted: Vec<WorkflowAuthorityFinding>,
    expired: Vec<ExpiredException>,
    drifted: Vec<DriftedException>,
    unused: Vec<UnusedException>,
}

fn authority_finding_identity(finding: &WorkflowAuthorityFinding) -> String {
    authority_exceptions::identity_of(
        &finding.workflow,
        &finding.job,
        &finding.step,
        &finding.capability,
        &finding.trigger,
        &finding.repository_boundary,
    )
}

/// Reconcile detector authority findings against ledger records.
///
/// Pure over plain data — findings, records, and the reconciliation date — so
/// every state is unit-testable without a filesystem fixture. Every state that
/// is not "exactly one valid, unexpired record" is reported and blocks.
fn reconcile_authority_exceptions(
    findings: &[WorkflowAuthorityFinding],
    records: &[AuthorityException],
    today: NaiveDate,
) -> AuthorityReconciliation {
    let mut out = AuthorityReconciliation::default();
    let mut consumed = vec![false; records.len()];
    // A record naming an unparsed authority shape is rejected before matching
    // begins, so it can neither authorize nor absorb a finding. The finding it
    // aimed at stays unexcepted and the record itself falls out as unused.
    let rejected: Vec<bool> = records
        .iter()
        .map(|record| record.capability.starts_with(UNKNOWN_PERMISSION_PREFIX))
        .collect();

    for finding in findings {
        let identity = authority_finding_identity(finding);
        let exact: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(index, record)| !rejected[*index] && record.identity() == identity)
            .map(|(index, _)| index)
            .collect();

        if let Some(&first) = exact.first() {
            for &index in &exact {
                consumed[index] = true;
            }
            let record = &records[first];
            // An unparseable review date is treated as expired: a record whose
            // lifecycle cannot be read has no lifecycle.
            let live = NaiveDate::parse_from_str(record.review_after.trim(), "%Y-%m-%d")
                .is_ok_and(|review_after| review_after >= today);
            if live {
                out.authorized.push(AuthorizedException {
                    workflow: finding.workflow.clone(),
                    job: finding.job.clone(),
                    step: finding.step.clone(),
                    capability: finding.capability.clone(),
                    trigger: finding.trigger.clone(),
                    repository_boundary: finding.repository_boundary.clone(),
                    repository: record.repository.clone(),
                    owner: record.owner.clone(),
                    review_after: record.review_after.clone(),
                });
            } else {
                out.expired.push(ExpiredException {
                    workflow: finding.workflow.clone(),
                    job: finding.job.clone(),
                    step: finding.step.clone(),
                    capability: finding.capability.clone(),
                    trigger: finding.trigger.clone(),
                    repository_boundary: finding.repository_boundary.clone(),
                    owner: record.owner.clone(),
                    review_after: record.review_after.clone(),
                    today: today.format("%Y-%m-%d").to_string(),
                });
            }
            continue;
        }

        let partial: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(index, record)| {
                !rejected[*index]
                    && record.workflow == finding.workflow
                    && record.job == finding.job
                    && record.step == finding.step
                    && record.capability == finding.capability
            })
            .map(|(index, _)| index)
            .collect();

        if partial.is_empty() {
            out.unexcepted.push(finding.clone());
            continue;
        }

        for index in partial {
            consumed[index] = true;
            let record = &records[index];
            let mut drifted = Vec::new();
            if record.trigger != finding.trigger {
                drifted.push(DriftedField {
                    field: "trigger",
                    expected: record.trigger.clone(),
                    actual: finding.trigger.clone(),
                });
            }
            if record.finding_repository_boundary != finding.repository_boundary {
                drifted.push(DriftedField {
                    field: "finding_repository_boundary",
                    expected: record.finding_repository_boundary.clone(),
                    actual: finding.repository_boundary.clone(),
                });
            }
            out.drifted.push(DriftedException {
                workflow: finding.workflow.clone(),
                job: finding.job.clone(),
                step: finding.step.clone(),
                capability: finding.capability.clone(),
                owner: record.owner.clone(),
                drifted,
                remediation:
                    "re-review the capability and rewrite the exact record, or remove the authority"
                        .to_string(),
            });
        }
    }

    for (index, record) in records.iter().enumerate() {
        if consumed[index] {
            continue;
        }
        let reason = if rejected[index] {
            "a record may not authorize an unparsed authority shape; fix the workflow permissions instead".to_string()
        } else {
            "no detector authority finding matches this record; delete it".to_string()
        };
        out.unused.push(UnusedException {
            workflow: record.workflow.clone(),
            job: record.job.clone(),
            step: record.step.clone(),
            capability: record.capability.clone(),
            trigger: record.trigger.clone(),
            finding_repository_boundary: record.finding_repository_boundary.clone(),
            owner: record.owner.clone(),
            review_after: record.review_after.clone(),
            reason,
        });
    }

    out
}

/// Load and validate the ledger, reporting failure as a finding.
///
/// A ledger that will not parse or will not validate is a blocking state, but
/// it must not abort the run: the rest of the workflow report still has to
/// render so an operator can see the whole picture.
fn load_authority_records(
    workspace_root: &Path,
    today: Option<NaiveDate>,
) -> (Vec<AuthorityException>, Vec<String>) {
    let mut invalid = Vec::new();
    let doc = match authority_exceptions::load(workspace_root) {
        Ok(doc) => doc,
        Err(error) => {
            invalid.push(format!("{error:#}"));
            return (Vec::new(), invalid);
        }
    };
    match today {
        Some(today) => {
            if let Err(error) = authority_exceptions::validate_doc(&doc, today, |workflow| {
                workspace_root.join(workflow).is_file()
            }) {
                invalid.push(format!("{error:#}"));
                // A record that failed validation may not authorize anything.
                // Returning it anyway let the report print ACCEPTED for a
                // rejected record — and in advisory mode the report is the only
                // output, so it would endorse exactly what the validator
                // refused. Drop the ledger so every finding reports unexcepted.
                return (Vec::new(), invalid);
            }
        }
        None => {
            invalid.push("could not resolve today's date to validate the ledger".to_string());
            return (Vec::new(), invalid);
        }
    }
    (doc.authority_exception, invalid)
}

fn workflow_name(yaml_text: &str) -> Option<String> {
    yaml_text.lines().find_map(|line| {
        let without_comment = strip_yaml_inline_comment(line);
        if without_comment.len() != without_comment.trim_start().len() {
            return None;
        }
        let trimmed = without_comment.trim();
        trimmed
            .strip_prefix("name:")
            .map(|name| name.trim().trim_matches(['\'', '"']).to_string())
    })
}

fn workflow_trigger_names(yaml_text: &str) -> BTreeSet<String> {
    let mut triggers = BTreeSet::new();
    let mut in_on = false;
    let mut types_indent = None;
    for line in yaml_text.lines() {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim();
        let indent = without_comment.len() - without_comment.trim_start().len();
        if indent == 0 && trimmed.starts_with("on:") {
            in_on = true;
            if let Some(value) = trimmed.strip_prefix("on:") {
                let inline = value.trim().trim_start_matches('[').trim_end_matches(']');
                for trigger in inline.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    triggers.insert(trigger.trim_matches(['\'', '"']).to_string());
                }
            }
            if trimmed != "on:" {
                in_on = false;
            }
            continue;
        }
        if indent == 0 && in_on {
            break;
        }
        if !in_on || trimmed.starts_with('#') {
            continue;
        }
        if let Some(parent_indent) = types_indent {
            if indent <= parent_indent {
                types_indent = None;
            } else if trimmed
                .strip_prefix('-')
                .is_some_and(|value| value.trim().trim_matches(['\'', '"']) == "labeled")
            {
                triggers.insert("labeled".to_string());
            }
        }
        if let Some(value) = trimmed.strip_prefix("types:") {
            types_indent = Some(indent);
            let inline = value.trim().trim_matches(['[', ']']);
            if inline
                .split(',')
                .map(str::trim)
                .map(|value| value.trim_matches(['\'', '"']))
                .any(|value| value == "labeled")
            {
                triggers.insert("labeled".to_string());
            }
        }
        if indent != 2 {
            continue;
        }
        if let Some(name) = trimmed.strip_suffix(':') {
            triggers.insert(name.trim().to_string());
        } else if let Some((name, _)) = trimmed.split_once(':') {
            triggers.insert(name.trim().to_string());
        }
    }
    triggers
}

fn workflow_has_tag_trigger(yaml_text: &str) -> bool {
    let triggers = workflow_trigger_names(yaml_text);
    if !triggers.contains("push") {
        return false;
    }
    yaml_text.lines().any(|line| {
        let trimmed = strip_yaml_inline_comment(line).trim();
        trimmed == "tags:" || trimmed.starts_with("tags:")
    })
}

fn has_true_cancel_in_progress(yaml_text: &str) -> bool {
    yaml_text.lines().any(|line| {
        let trimmed = strip_yaml_inline_comment(line).trim();
        trimmed
            .strip_prefix("cancel-in-progress:")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
    })
}

fn temporary_workflow_identity(path: &str, name: &str) -> bool {
    let path_lower = path.to_ascii_lowercase();
    let stem = path_lower
        .rsplit('/')
        .next()
        .unwrap_or(&path_lower)
        .trim_end_matches(".yml");
    let path_words: Vec<&str> = stem
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let path_marker = (stem == "_temp" || stem.starts_with("_temp-") || stem.starts_with("_temp_"))
        || path_words
            .first()
            .is_some_and(|word| matches!(*word, "temp" | "temporary"))
        || path_words.starts_with(&["one", "off"])
        || path_words.starts_with(&["proof", "pulse"]);
    let words: Vec<String> = name
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    let named_marker = words
        .first()
        .is_some_and(|word| matches!(word.as_str(), "temp" | "temporary"))
        || words.starts_with(&["one".to_string(), "off".to_string()])
        || words.starts_with(&["proof".to_string(), "pulse".to_string()]);
    path_marker || named_marker
}

fn temporary_branch_filters(yaml_text: &str) -> Vec<String> {
    let markers = ["temp", "temporary", "repair", "proof-pulse", "one-off"];
    let mut found = BTreeSet::new();
    let lines: Vec<&str> = yaml_text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim();
        if !trimmed.starts_with("branches:") {
            continue;
        }
        let indent = without_comment.len() - without_comment.trim_start().len();
        let inline = trimmed.strip_prefix("branches:").unwrap_or_default();
        if let Some(marker) = markers.iter().find(|marker| inline.contains(**marker)) {
            found.insert((*marker).to_string());
        }
        for branch_line in lines.iter().skip(index + 1) {
            let branch_without_comment = strip_yaml_inline_comment(branch_line);
            let branch_trimmed = branch_without_comment.trim();
            if branch_trimmed.is_empty() {
                continue;
            }
            let branch_indent =
                branch_without_comment.len() - branch_without_comment.trim_start().len();
            if branch_indent <= indent {
                break;
            }
            if let Some(marker) = markers
                .iter()
                .find(|marker| branch_trimmed.contains(**marker))
            {
                found.insert((*marker).to_string());
            }
        }
    }
    found.into_iter().collect()
}

fn workflow_permission_scopes(
    yaml_text: &str,
    job_blocks: &[(String, String)],
) -> Vec<PermissionScope> {
    let mut scopes = Vec::new();
    let lines: Vec<&str> = yaml_text.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let without_comment = strip_yaml_inline_comment(lines[index]);
        let trimmed = without_comment.trim();
        let indent = without_comment.len() - without_comment.trim_start().len();
        if indent == 0 && trimmed.starts_with("permissions:") {
            scopes.push(PermissionScope {
                job: "<workflow>".to_string(),
                values: parse_permission_mapping(&lines, index, indent),
            });
            break;
        }
        index += 1;
    }
    for (job, block) in job_blocks {
        scopes.extend(workflow_permission_scopes_for_job(block, job));
    }
    scopes
}

fn workflow_permission_scopes_for_job(block: &str, job: &str) -> Vec<PermissionScope> {
    let lines: Vec<&str> = block.lines().collect();
    let Some(first) = lines.first() else {
        return Vec::new();
    };
    let job_indent = first.len() - first.trim_start().len();
    for (index, line) in lines.iter().enumerate() {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim();
        let indent = without_comment.len() - without_comment.trim_start().len();
        if indent == job_indent + 2 && trimmed.starts_with("permissions:") {
            return vec![PermissionScope {
                job: job.to_string(),
                values: parse_permission_mapping(&lines, index, indent),
            }];
        }
    }
    Vec::new()
}

fn parse_nested_mapping(
    lines: &[&str],
    start: usize,
    parent_indent: usize,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in lines.iter().skip(start + 1) {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = without_comment.len() - without_comment.trim_start().len();
        if indent <= parent_indent {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            values.insert(key.trim().to_string(), value.trim().to_ascii_lowercase());
        }
    }
    values
}

fn parse_permission_mapping(
    lines: &[&str],
    start: usize,
    parent_indent: usize,
) -> BTreeMap<String, String> {
    let declaration = strip_yaml_inline_comment(lines[start]);
    let value = declaration
        .trim()
        .strip_prefix("permissions:")
        .map(str::trim)
        .unwrap_or_default();
    if value.is_empty() {
        return parse_nested_mapping(lines, start, parent_indent);
    }

    let mut values = BTreeMap::new();
    let normalized = value.trim_matches(['{', '}', '\'', '"']).trim();
    if normalized.is_empty() {
        return values;
    }
    if normalized.eq_ignore_ascii_case("write-all") {
        values.insert("*".to_string(), "write".to_string());
    } else if normalized.eq_ignore_ascii_case("read-all") {
        values.insert("*".to_string(), "read".to_string());
    } else {
        for pair in normalized.split(',') {
            if let Some((key, permission)) = pair.split_once(':') {
                values.insert(
                    key.trim().trim_matches(['\'', '"']).to_string(),
                    permission
                        .trim()
                        .trim_matches(['\'', '"'])
                        .to_ascii_lowercase(),
                );
            }
        }
        if values.is_empty() {
            values.insert(
                "*".to_string(),
                format!("unknown:{}", normalized.to_ascii_lowercase()),
            );
        }
    }
    values
}

fn scope_has_write_permission(scopes: &[PermissionScope]) -> bool {
    scopes.iter().any(|scope| {
        scope
            .values
            .values()
            .any(|value| value == "write" || value == "*")
    })
}

fn contains_release_authority(text: &str) -> bool {
    workflow_step_blocks(text)
        .iter()
        .any(|step| step_contains_release_authority(&step.content))
}

fn workflow_contains_release_authority(yaml_text: &str) -> bool {
    let job_blocks = workflow_job_blocks(yaml_text);
    contains_release_authority(yaml_text)
        || job_blocks
            .iter()
            .any(|(_, block)| contains_release_authority(block))
        || workflow_permission_scopes(yaml_text, &job_blocks)
            .iter()
            .any(|scope| scope.values.get("id-token").is_some_and(|v| v == "write"))
}

fn step_contains_release_authority(content: &str) -> bool {
    let lower = uncommented_workflow_text(content).to_ascii_lowercase();
    shell_segments(&lower).iter().any(|segment| {
        shell_starts_with_command(segment, "cargo publish")
            || shell_starts_with_command(segment, "gh release")
            || (shell_contains_command(segment, "git tag") && git_tag_mutates(segment))
            || shell_starts_with_command(segment, "cosign")
            || segment.contains("$cargo_registry_token")
    })
}

fn has_dispatch_ref_input(yaml_text: &str) -> bool {
    let lines: Vec<&str> = yaml_text.lines().collect();
    let mut in_on = false;
    let mut trigger_indent = None;
    let mut in_dispatch = false;
    let mut inputs_indent = None;
    let mut input_indent = None;
    for line in lines {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim();
        let indent = without_comment.len() - without_comment.trim_start().len();
        if indent == 0 && trimmed.starts_with("on:") {
            in_on = true;
            continue;
        }
        if !in_on {
            continue;
        }
        if indent == 0 {
            break;
        }
        if trigger_indent.is_none() {
            if trimmed.is_empty() {
                continue;
            }
            trigger_indent = Some(indent);
        }
        if Some(indent) == trigger_indent {
            in_dispatch = trimmed.starts_with("workflow_dispatch:");
            inputs_indent = None;
            input_indent = None;
            continue;
        }
        if !in_dispatch {
            continue;
        }
        if inputs_indent.is_none() && indent > trigger_indent.unwrap_or(indent) {
            if trimmed.starts_with("inputs:") {
                inputs_indent = Some(indent);
                input_indent = None;
            }
            continue;
        }
        if let Some(parent_indent) = inputs_indent {
            if indent <= parent_indent {
                inputs_indent = None;
                input_indent = None;
                continue;
            }
            if input_indent.is_none() {
                input_indent = Some(indent);
                if trimmed.starts_with("ref:") {
                    return true;
                }
                continue;
            }
            if Some(indent) == input_indent && trimmed.starts_with("ref:") {
                return true;
            }
        }
    }
    false
}

fn has_exact_release_identity_language(yaml_text: &str) -> bool {
    workflow_job_blocks(yaml_text).iter().any(|(_, block)| {
        workflow_step_blocks(block).iter().any(|step| {
            let lower = step.content.to_ascii_lowercase();
            shell_segments(&lower)
                .iter()
                .any(|segment| shell_starts_with_command(segment, "cargo xtask release-identity"))
        })
    })
}

fn workflow_step_blocks(job_block: &str) -> Vec<WorkflowStepBlock> {
    let lines: Vec<&str> = job_block.lines().collect();
    let Some((steps_index, steps_indent)) = lines.iter().enumerate().find_map(|(index, line)| {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim();
        let indent = without_comment.len() - without_comment.trim_start().len();
        (trimmed == "steps:").then_some((index, indent))
    }) else {
        return Vec::new();
    };

    let mut steps = Vec::new();
    let mut current: Option<WorkflowStepBlock> = None;
    for line in lines.into_iter().skip(steps_index + 1) {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim();
        let indent = without_comment.len() - without_comment.trim_start().len();
        if !trimmed.is_empty() && indent <= steps_indent {
            break;
        }
        if indent == steps_indent + 2 && trimmed.starts_with("- ") {
            if let Some(step) = current.take() {
                steps.push(step);
            }
            let label = trimmed
                .strip_prefix("- name:")
                .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
                .or_else(|| {
                    trimmed
                        .strip_prefix("- uses:")
                        .map(|value| value.trim().to_string())
                })
                .unwrap_or_else(|| "<unnamed>".to_string());
            current = Some(WorkflowStepBlock {
                name: label,
                content: String::new(),
            });
        }
        if let Some(step) = current.as_mut() {
            step.content.push_str(line);
            step.content.push('\n');
        }
    }
    if let Some(step) = current {
        steps.push(step);
    }
    steps
}

fn self_mutation_capabilities(content: &str) -> Vec<&'static str> {
    let code: String = content
        .lines()
        .map(strip_yaml_inline_comment)
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let lower = code.to_ascii_lowercase();
    let segments = shell_segments(&lower);
    let mut capabilities = Vec::new();
    for (command, capability) in [
        ("git add", "git-add"),
        ("git commit", "git-commit"),
        ("git push", "git-push"),
        ("git update-ref", "git-ref-mutation"),
        ("git tag", "git-tag-mutation"),
        ("gh api", "github-api-mutation"),
    ] {
        let detected = segments.iter().any(|segment| {
            if !shell_contains_command(segment, command) {
                return false;
            }
            if command == "gh api" {
                return gh_api_mutates(segment);
            }
            if command == "git tag" {
                return git_tag_mutates(segment);
            }
            true
        });
        if detected {
            capabilities.push(capability);
        }
    }
    let deletes_workflow = segments.iter().any(|segment| {
        let delete_command = shell_starts_with_command(segment, "rm")
            || shell_starts_with_command(segment, "git rm")
            || shell_starts_with_command(segment, "remove-item")
            || shell_starts_with_command(segment, "del");
        delete_command && segment.contains(".github/workflows/")
    });
    if deletes_workflow {
        capabilities.push("workflow-self-deletion");
    }
    capabilities
}

fn shell_segments(text: &str) -> Vec<String> {
    splice_line_continuations(text)
        .split(['\n', ';', '|', '&', '(', ')', '`'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// Join shell line continuations so a command written across several lines stays
/// one segment. Without this, `gh api repos/x \` + `--method post` splits into two
/// segments and the mutation flag is never attributed to the invocation.
fn splice_line_continuations(text: &str) -> String {
    let mut spliced = String::with_capacity(text.len());
    let mut lines = text.lines().peekable();
    let mut continued = false;
    while let Some(line) = lines.next() {
        // A continued line's indentation is layout, not a shell argument separator.
        let line = if continued { line.trim_start() } else { line };
        if let Some(head) = line.trim_end().strip_suffix('\\') {
            spliced.push_str(head.trim_end());
            spliced.push(' ');
            continued = true;
            continue;
        }
        continued = false;
        spliced.push_str(line);
        if lines.peek().is_some() {
            spliced.push('\n');
        }
    }
    spliced
}

fn shell_contains_command(text: &str, command: &str) -> bool {
    shell_starts_with_command(text, command)
}

fn shell_starts_with_command(segment: &str, command: &str) -> bool {
    let mut candidate = segment.trim();
    loop {
        let next = candidate
            .strip_prefix("if ")
            .or_else(|| candidate.strip_prefix("then "))
            .or_else(|| candidate.strip_prefix("do "))
            .or_else(|| candidate.strip_prefix("sudo "))
            .or_else(|| candidate.strip_prefix("run:"))
            .or_else(|| candidate.strip_prefix("!"))
            .or_else(|| candidate.strip_prefix("("))
            .or_else(|| candidate.strip_prefix("{"));
        let Some(next) = next else {
            break;
        };
        candidate = next.trim_start();
    }
    while let Some(first) = candidate.split_whitespace().next() {
        if !first.contains('=') || first.starts_with('=') {
            break;
        }
        let Some(offset) = candidate.find(first) else {
            break;
        };
        candidate = candidate[offset + first.len()..].trim_start();
    }
    candidate == command
        || candidate
            .strip_prefix(command)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

fn gh_api_mutates(lowercase_content: &str) -> bool {
    const METHODS: [&str; 8] = [
        "--method post",
        "--method put",
        "--method patch",
        "--method delete",
        "-x post",
        "-x put",
        "-x patch",
        "-x delete",
    ];
    lowercase_content.match_indices("gh api").any(|(index, _)| {
        let invocation = &lowercase_content[index..];
        let end = invocation
            .find([';', '|', '&', '\n'])
            .unwrap_or(invocation.len());
        METHODS
            .iter()
            .any(|method| invocation[..end].contains(method))
    })
}

fn git_tag_mutates(lowercase_content: &str) -> bool {
    let Some(tag_index) = lowercase_content.find("git tag") else {
        return false;
    };
    let arguments: Vec<&str> = lowercase_content[tag_index + "git tag".len()..]
        .split_whitespace()
        .collect();
    let Some(first_argument) = arguments.first() else {
        // Bare `git tag` lists refs.
        return false;
    };
    if arguments
        .iter()
        .any(|argument| git_tag_argument_kind(argument) == GitTagArgument::Mutating)
    {
        return true;
    }
    git_tag_argument_kind(first_argument) != GitTagArgument::ReadOnly
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GitTagArgument {
    Mutating,
    ReadOnly,
    Unknown,
}

/// Classify a `git tag` argument. Listing forms only read refs; create/delete
/// forms mutate them. Anything unrecognised stays `Unknown` so the caller keeps
/// failing closed and reports mutation.
fn git_tag_argument_kind(argument: &str) -> GitTagArgument {
    const MUTATING: [&str; 12] = [
        "-a",
        "-s",
        "-d",
        "-f",
        "-m",
        "-u",
        "--annotate",
        "--sign",
        "--delete",
        "--force",
        "--message",
        "--file",
    ];
    const READ_ONLY: [&str; 11] = [
        "-l",
        "-i",
        "--list",
        "--sort",
        "--contains",
        "--no-contains",
        "--points-at",
        "--merged",
        "--no-merged",
        "--format",
        "--column",
    ];
    let name = argument.split_once('=').map_or(argument, |(name, _)| name);
    if MUTATING.contains(&name) {
        return GitTagArgument::Mutating;
    }
    // `-n` and `-n5` print annotation lines while listing.
    if READ_ONLY.contains(&name)
        || (name.starts_with("-n") && name[2..].chars().all(|c| c.is_ascii_digit()))
    {
        return GitTagArgument::ReadOnly;
    }
    GitTagArgument::Unknown
}

fn block_has_untrusted_checkout(block: &str) -> bool {
    let lower = uncommented_workflow_text(block).to_ascii_lowercase();
    lower.contains("actions/checkout")
        && (lower.contains("github.event.pull_request")
            || lower.contains("refs/pull/")
            || lower.contains("head.sha"))
}

pub(crate) fn uncommented_workflow_text(text: &str) -> String {
    text.lines()
        .map(strip_yaml_inline_comment)
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn workflow_jobs_missing_repository_guard(
    yaml_text: &str,
    required_repository: &str,
) -> Vec<String> {
    workflow_job_blocks(yaml_text)
        .into_iter()
        .filter_map(|(job, block)| {
            if block_has_repository_guard(&block, required_repository) {
                None
            } else {
                Some(job)
            }
        })
        .collect()
}

pub(crate) fn workflow_job_blocks(yaml_text: &str) -> Vec<(String, String)> {
    let mut jobs = Vec::new();
    let mut in_jobs = false;
    let mut jobs_indent = 0usize;
    let mut current_job: Option<String> = None;
    let mut current_block = String::new();

    for line in yaml_text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if current_job.is_some() {
                current_block.push_str(line);
                current_block.push('\n');
            }
            continue;
        }

        let indent = line.len() - trimmed.len();
        if !in_jobs {
            if trimmed == "jobs:" {
                in_jobs = true;
                jobs_indent = indent;
            }
            continue;
        }

        if indent <= jobs_indent {
            break;
        }

        let is_job_key = indent == jobs_indent + 2
            && trimmed.ends_with(':')
            && !trimmed.starts_with('-')
            && !trimmed.contains(' ');
        if is_job_key {
            if let Some(job) = current_job.take() {
                jobs.push((job, std::mem::take(&mut current_block)));
            }
            current_job = Some(trimmed.trim_end_matches(':').to_string());
            current_block.push_str(line);
            current_block.push('\n');
            continue;
        }

        if current_job.is_some() {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    if let Some(job) = current_job {
        jobs.push((job, current_block));
    }

    jobs
}

fn block_has_repository_guard(block: &str, required_repository: &str) -> bool {
    let Some(expression) = job_level_if_expression(block) else {
        return false;
    };
    let single_quoted = format!("github.repository == '{required_repository}'");
    let double_quoted = format!("github.repository == \"{required_repository}\"");
    expression.contains(&single_quoted) || expression.contains(&double_quoted)
}

pub(crate) fn job_level_if_expression(block: &str) -> Option<String> {
    let lines: Vec<&str> = block.lines().collect();
    let job_indent = lines.iter().find_map(|line| {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            None
        } else {
            Some(without_comment.len() - trimmed.len())
        }
    })?;
    let field_indent = job_indent + 2;

    let mut index = 0usize;
    while index < lines.len() {
        let without_comment = strip_yaml_inline_comment(lines[index]);
        let trimmed = without_comment.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }

        let indent = without_comment.len() - trimmed.len();
        if indent == field_indent && trimmed.starts_with("if:") {
            let value = trimmed.trim_start_matches("if:").trim();
            if is_yaml_block_scalar(value) || value.is_empty() {
                return Some(collect_yaml_continuation_expression(
                    &lines,
                    index + 1,
                    field_indent,
                ));
            }
            let continuation =
                collect_yaml_continuation_expression(&lines, index + 1, field_indent);
            if continuation.is_empty() {
                return Some(value.to_string());
            }
            return Some(format!("{value} {continuation}"));
        }

        index += 1;
    }

    None
}

fn is_yaml_block_scalar(value: &str) -> bool {
    matches!(value.chars().next(), Some('|' | '>'))
}

fn collect_yaml_continuation_expression(
    lines: &[&str],
    start: usize,
    parent_indent: usize,
) -> String {
    let mut expression = String::new();

    for line in &lines[start..] {
        let without_comment = strip_yaml_inline_comment(line);
        let trimmed = without_comment.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let indent = without_comment.len() - trimmed.len();
        if indent <= parent_indent {
            break;
        }

        expression.push_str(trimmed);
        expression.push(' ');
    }

    expression
}

fn strip_yaml_inline_comment(line: &str) -> &str {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut previous_was_whitespace = true;

    for (index, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                previous_was_whitespace = false;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                previous_was_whitespace = false;
            }
            '#' if !in_single_quote && !in_double_quote && previous_was_whitespace => {
                return &line[..index];
            }
            _ => {
                previous_was_whitespace = ch.is_whitespace();
            }
        }
    }

    line
}

// ─── check-process-policy ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct PerWorkflowReport {
    workflow: String,
    declared_profile: String,
    detected: Vec<String>,
    unknown: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ScanReport {
    tool: &'static str,
    mode: &'static str,
    today: String,
    summary: ScanSummary,
    findings: ProfileLifecycleFindings,
    workflows: Vec<PerWorkflowReport>,
}

#[derive(Debug, Clone, Serialize)]
struct ScanSummary {
    workflows: usize,
    profiles: usize,
    unknown_total: usize,
    missing_fields: usize,
    invalid_profiles: usize,
    stale: usize,
    orphaned: usize,
}

/// Well-known shell-command tokens we look for inside workflow contents.
/// This list is the recognition surface; commands that appear here but are
/// not in the workflow's declared process profile are flagged as "unknown".
/// Tokens outside this list are silently ignored (start-simple posture).
const KNOWN_COMMANDS: &[&str] = &[
    "cargo",
    "rustup",
    "rustc",
    "cargo-fuzz",
    "cargo-mutants",
    "cargo-llvm-cov",
    "cargo-nextest",
    "shipper",
    "gh",
    "tar",
    "sha256sum",
    "install",
    "sudo",
    "bash",
    "curl",
    "wget",
    "sh",
    "bun",
    "node",
    "npm",
    "python",
    "python3",
    "pip",
    "docker",
    "kubectl",
    "make",
    "mkdir",
    "cat",
    "jq",
];

pub fn check_process_policy(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;
    check_process_policy_at(&workspace_root, mode)
}

fn check_process_policy_at(workspace_root: &Path, mode: Mode) -> Result<()> {
    let entries = load_workflow_allowlist(workspace_root)?;
    let workflows = tracked_workflow_files(workspace_root)?;
    let today = today_date()?;
    let referenced = referenced_profiles(&entries, &workflows, ProfileKind::Process);
    let catalog = load_profile_catalog(workspace_root, PROCESS_ALLOWLIST, &referenced, today)?;
    let profiles_by_name = &catalog.profiles;

    let mut per_workflow = Vec::new();
    for e in live_workflow_entries(&entries, &workflows) {
        if is_dependabot_config(e) {
            // dependabot.yml is a config file, not a script — there are no
            // shell commands to scan for.
            continue;
        }
        let path = match &e.path {
            Some(p) => p,
            None => continue,
        };
        let profile = e.process_policy.clone().unwrap_or_default();
        let allowed: BTreeSet<String> = profiles_by_name
            .get(&profile)
            .map(|p| p.allowed_processes.iter().cloned().collect())
            .unwrap_or_default();

        let content = read_workflow_content(workspace_root, path).unwrap_or_default();
        let detected = detect_commands_in_runs(&content, KNOWN_COMMANDS);
        let unknown: Vec<String> = detected
            .iter()
            .filter(|c| !allowed.contains(c.as_str()))
            .cloned()
            .collect();
        per_workflow.push(PerWorkflowReport {
            workflow: path.clone(),
            declared_profile: profile,
            detected,
            unknown,
        });
    }

    finish_profile_scan(
        workspace_root,
        "cargo xtask check-process-policy",
        "process-policy-report",
        mode,
        today,
        catalog,
        per_workflow,
    )
}

// ─── check-network-policy ───────────────────────────────────────────────────

pub fn check_network_policy(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;
    check_network_policy_at(&workspace_root, mode)
}

fn check_network_policy_at(workspace_root: &Path, mode: Mode) -> Result<()> {
    let entries = load_workflow_allowlist(workspace_root)?;
    let workflows = tracked_workflow_files(workspace_root)?;
    let today = today_date()?;
    let referenced = referenced_profiles(&entries, &workflows, ProfileKind::Network);
    let catalog = load_profile_catalog(workspace_root, NETWORK_ALLOWLIST, &referenced, today)?;
    let profiles_by_name = &catalog.profiles;
    let host_re =
        Regex::new(r"https?://([A-Za-z0-9.\-]+)").context("compiling network hostname regex")?;

    let mut per_workflow = Vec::new();
    for e in live_workflow_entries(&entries, &workflows) {
        if is_dependabot_config(e) {
            // dependabot.yml is configuration, not a script — no URLs to scan.
            continue;
        }
        let path = match &e.path {
            Some(p) => p,
            None => continue,
        };
        let profile = e.network_policy.clone().unwrap_or_default();
        let allowed: BTreeSet<String> = profiles_by_name
            .get(&profile)
            .map(|p| p.allowed_endpoints.iter().cloned().collect())
            .unwrap_or_default();

        let content = read_workflow_content(workspace_root, path).unwrap_or_default();
        let mut detected: BTreeSet<String> = BTreeSet::new();
        for caps in host_re.captures_iter(&content) {
            if let Some(host) = caps.get(1) {
                detected.insert(host.as_str().to_string());
            }
        }
        let detected_vec: Vec<String> = detected.into_iter().collect();
        let unknown: Vec<String> = detected_vec
            .iter()
            .filter(|h| !endpoint_covered(h, &allowed))
            .cloned()
            .collect();
        per_workflow.push(PerWorkflowReport {
            workflow: path.clone(),
            declared_profile: profile,
            detected: detected_vec,
            unknown,
        });
    }

    finish_profile_scan(
        workspace_root,
        "cargo xtask check-network-policy",
        "network-policy-report",
        mode,
        today,
        catalog,
        per_workflow,
    )
}

fn finish_profile_scan(
    workspace_root: &Path,
    tool: &'static str,
    report_basename: &str,
    mode: Mode,
    today: NaiveDate,
    catalog: ProfileCatalog,
    workflows: Vec<PerWorkflowReport>,
) -> Result<()> {
    let unknown_total = workflows
        .iter()
        .map(|workflow| workflow.unknown.len())
        .sum();
    let report = ScanReport {
        tool,
        mode: mode_str(mode),
        today: today.format("%Y-%m-%d").to_string(),
        summary: ScanSummary {
            workflows: workflows.len(),
            profiles: catalog.entries,
            unknown_total,
            missing_fields: catalog.findings.missing_fields.len(),
            invalid_profiles: catalog.findings.invalid_profiles.len(),
            stale: catalog.findings.stale.len(),
            orphaned: catalog.findings.orphaned.len(),
        },
        findings: catalog.findings,
        workflows,
    };
    write_scan_report(workspace_root, report_basename, &report)?;
    println!(
        "{} ({}): workflows={} profiles={} unknown_total={} missing_fields={} invalid_profiles={} stale={} orphaned={}",
        report.tool,
        report.mode,
        report.summary.workflows,
        report.summary.profiles,
        report.summary.unknown_total,
        report.summary.missing_fields,
        report.summary.invalid_profiles,
        report.summary.stale,
        report.summary.orphaned,
    );

    let blocking = profile_blocking_count(mode, &report.findings, unknown_total);
    if blocking > 0 && !matches!(mode, Mode::Advisory) {
        bail!(
            "{}: {} mode found {} blocking issue(s) across {} workflow(s)",
            report.tool,
            report.mode,
            blocking,
            report.summary.workflows
        );
    }
    Ok(())
}

fn endpoint_covered(host: &str, allowed: &BTreeSet<String>) -> bool {
    // Exact match, or `host` is a subdomain of an allowed entry.
    if allowed.contains(host) {
        return true;
    }
    allowed.iter().any(|a| {
        host == a || host.ends_with(&format!(".{}", a)) || a.ends_with(&format!(".{}", host))
    })
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn tracked_workflow_files(workspace_root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("ls-files")
        .arg("-z")
        .output()
        .context("running `git ls-files -z`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`git ls-files -z` exited {}: {}",
            output.status,
            stderr.trim()
        );
    }
    let mut paths: Vec<String> = output
        .stdout
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .filter(|p| p.starts_with(".github/workflows/") && p.ends_with(".yml"))
        .collect();
    paths.sort();
    Ok(paths)
}

fn load_workflow_allowlist(workspace_root: &Path) -> Result<Vec<RawWorkflowEntry>> {
    let path = workspace_root.join(WORKFLOW_ALLOWLIST);
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc: WorkflowAllowlistDoc =
        toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))?;
    Ok(doc.workflow)
}

fn load_profile_names(workspace_root: &Path, rel: &str) -> Result<BTreeSet<String>> {
    let path = workspace_root.join(rel);
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc: ProfileDoc =
        toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))?;
    Ok(doc.profile.into_iter().filter_map(|p| p.name).collect())
}

fn load_profile_catalog(
    workspace_root: &Path,
    rel: &str,
    referenced: &BTreeSet<String>,
    today: NaiveDate,
) -> Result<ProfileCatalog> {
    let path = workspace_root.join(rel);
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc: ProfileDoc =
        toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))?;
    Ok(reconcile_profiles(doc.profile, referenced, today))
}

fn reconcile_profiles(
    raw_profiles: Vec<RawProfile>,
    referenced: &BTreeSet<String>,
    today: NaiveDate,
) -> ProfileCatalog {
    let entries = raw_profiles.len();
    let mut findings = ProfileLifecycleFindings::default();
    let mut name_counts = BTreeMap::<String, usize>::new();
    for raw in &raw_profiles {
        if let Some(name) = nonblank(raw.name.as_deref()) {
            *name_counts.entry(name.to_string()).or_default() += 1;
        }
    }
    for (name, count) in &name_counts {
        if *count > 1 {
            findings.invalid_profiles.push(InvalidProfile {
                profile: name.clone(),
                field: "name",
                value: name.clone(),
                reason: format!("duplicate profile name appears {count} times"),
            });
        }
    }

    let mut profiles = BTreeMap::new();
    for (index, raw) in raw_profiles.into_iter().enumerate() {
        let label = nonblank(raw.name.as_deref())
            .map(str::to_string)
            .unwrap_or_else(|| format!("profile[{index}]"));
        let missing = profile_missing_fields(&raw);
        if !missing.is_empty() {
            findings.missing_fields.push(ProfileMissingFields {
                profile: label,
                missing,
            });
            continue;
        }
        let name = raw.name.as_deref().unwrap_or_default().trim().to_string();
        if name_counts.get(&name).copied().unwrap_or_default() > 1 {
            continue;
        }

        let created =
            parse_canonical_profile_date(&name, "created", raw.created.as_deref(), &mut findings);
        let review_after = parse_canonical_profile_date(
            &name,
            "review_after",
            raw.review_after.as_deref(),
            &mut findings,
        );
        let mut valid = created.is_some() && review_after.is_some();
        if let (Some(created), Some(review_after)) = (created, review_after) {
            if review_after < created {
                findings.invalid_profiles.push(InvalidProfile {
                    profile: name.clone(),
                    field: "review_after",
                    value: raw.review_after.clone().unwrap_or_default(),
                    reason: "review_after is before created".to_string(),
                });
                valid = false;
            } else if review_after < today {
                findings.stale.push(StaleProfile {
                    profile: name.clone(),
                    review_after: raw.review_after.clone().unwrap_or_default(),
                    today: today.format("%Y-%m-%d").to_string(),
                });
            }
        }
        if valid {
            profiles.insert(name, raw);
        }
    }

    findings.orphaned = profiles
        .keys()
        .filter(|name| !referenced.contains(name.as_str()))
        .cloned()
        .collect();
    ProfileCatalog {
        entries,
        profiles,
        findings,
    }
}

fn profile_blocking_count(
    mode: Mode,
    findings: &ProfileLifecycleFindings,
    unknown_total: usize,
) -> usize {
    let allowlist_blocking =
        unknown_total + findings.missing_fields.len() + findings.invalid_profiles.len();
    match mode {
        Mode::Advisory | Mode::BlockingAllowlist => allowlist_blocking,
        Mode::BlockingStrict => allowlist_blocking + findings.stale.len() + findings.orphaned.len(),
    }
}

fn profile_missing_fields(raw: &RawProfile) -> Vec<String> {
    [
        ("name", raw.name.as_deref()),
        ("owner", raw.owner.as_deref()),
        ("reason", raw.reason.as_deref()),
        ("created", raw.created.as_deref()),
        ("review_after", raw.review_after.as_deref()),
    ]
    .into_iter()
    .filter(|(_, value)| nonblank(*value).is_none())
    .map(|(field, _)| field.to_string())
    .collect()
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn parse_canonical_profile_date(
    profile: &str,
    field: &'static str,
    raw: Option<&str>,
    findings: &mut ProfileLifecycleFindings,
) -> Option<NaiveDate> {
    let raw = raw?;
    let parsed = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok();
    if let Some(date) = parsed
        && date.format("%Y-%m-%d").to_string() == raw
    {
        return Some(date);
    }
    findings.invalid_profiles.push(InvalidProfile {
        profile: profile.to_string(),
        field,
        value: raw.to_string(),
        reason: "expected canonical YYYY-MM-DD calendar date".to_string(),
    });
    None
}

fn live_workflow_entries<'a>(
    entries: &'a [RawWorkflowEntry],
    workflows: &[String],
) -> Vec<&'a RawWorkflowEntry> {
    let tracked: BTreeSet<&str> = workflows.iter().map(String::as_str).collect();
    entries
        .iter()
        .filter(|entry| !is_dependabot_config(entry))
        .filter(|entry| {
            entry
                .path
                .as_deref()
                .is_some_and(|path| tracked.contains(path))
        })
        .collect()
}

fn referenced_profiles(
    entries: &[RawWorkflowEntry],
    workflows: &[String],
    kind: ProfileKind,
) -> BTreeSet<String> {
    live_workflow_entries(entries, workflows)
        .into_iter()
        .filter_map(|entry| nonblank(kind.reference(entry)).map(str::to_string))
        .collect()
}

fn read_workflow_content(workspace_root: &Path, rel: &str) -> Result<String> {
    let path = workspace_root.join(rel);
    fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

/// Position-aware command detection inside `run:` blocks only.
///
/// The previous implementation grep-matched the whole YAML for known
/// command tokens, which produced false positives where a command name
/// appears as a cargo build target (`-p shipper`), an action ref
/// (`taiki-e/install-action`), or in a step `name:` line. This refined
/// scanner:
///
/// 1. Walks the YAML by indentation and picks out content under
///    `run:` keys — both inline (`run: cargo build`) and block scalars
///    (`run: |` followed by indented lines).
/// 2. Splits each run-block's content by shell statement separators
///    (newline, `;`, `&&`, `||`, `|`).
/// 3. Looks at the **first word** of each segment. Only that first
///    word can be a command in shell semantics; subsequent tokens are
///    arguments.
/// 4. Strips leading redirections and environment-variable assignments
///    (`FOO=bar cmd ...` ⇒ `cmd`).
///
/// `cargo build -p shipper` now flags `cargo` and nothing else.
/// `sudo apt-get install -y gcc` flags `sudo`.
/// `mkdir -p /tmp/x` flags `mkdir`.
fn detect_commands_in_runs(yaml_text: &str, vocabulary: &[&str]) -> Vec<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let vocab: BTreeSet<&str> = vocabulary.iter().copied().collect();

    let mut buffer = String::new();
    let mut in_run_block = false;
    let mut run_indent: usize = 0;

    for line in yaml_text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();

        // If we're inside a run block and the next line's indentation
        // returns to or below the run's `run:` key indent, the block ends.
        if in_run_block && !trimmed.is_empty() && indent <= run_indent {
            scan_run_content(&buffer, &vocab, &mut found);
            buffer.clear();
            in_run_block = false;
        }

        if let Some(rest) = trimmed.strip_prefix("run:") {
            // Flush any prior unterminated block (defensive).
            if in_run_block {
                scan_run_content(&buffer, &vocab, &mut found);
                buffer.clear();
            }
            in_run_block = true;
            run_indent = indent;
            let value = rest.trim();
            // Block-scalar markers: `|`, `>`, `|-`, `>-`, `|+`, `>+`.
            if !value.is_empty() && !matches!(value, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
                buffer.push_str(value);
                buffer.push('\n');
            }
            continue;
        }

        if in_run_block {
            // Skip blank lines (don't end the block; YAML allows them inside).
            if !trimmed.is_empty() {
                buffer.push_str(trimmed);
                buffer.push('\n');
            }
        }
    }
    if in_run_block && !buffer.is_empty() {
        scan_run_content(&buffer, &vocab, &mut found);
    }

    found.into_iter().collect()
}

fn scan_run_content(content: &str, vocab: &BTreeSet<&str>, found: &mut BTreeSet<String>) {
    // Split by shell separators. We don't try to honor quoted strings;
    // false-negatives there are acceptable (an attacker hiding a command
    // inside quoted strings would also need to break out of them, and the
    // policy stack assumes review).
    let separators: &[char] = &['\n', ';', '|', '&'];
    for raw_segment in content.split(separators) {
        let segment = raw_segment.trim();
        if segment.is_empty() {
            continue;
        }
        // Drop leading shell glue and env-var assignments.
        let mut tokens = segment.split_whitespace();
        let mut first = loop {
            match tokens.next() {
                Some(t) if t.contains('=') && !t.starts_with('=') => continue, // FOO=bar
                Some(t) if t == "\\" || t == "&&" || t == "||" => continue,
                Some(t) => break Some(t),
                None => break None,
            }
        };
        // Strip leading `(`, `!`, etc.
        while let Some(t) = first {
            let stripped = t.trim_start_matches(['(', '{', '!', ' ']);
            if stripped != t {
                first = Some(stripped);
                continue;
            }
            break;
        }
        if let Some(t) = first
            && vocab.contains(t)
        {
            found.insert(t.to_string());
        }
    }
}

fn is_dependabot_config(e: &RawWorkflowEntry) -> bool {
    e.kind.as_deref() == Some("dependabot_config")
}

fn write_scan_report(workspace_root: &Path, basename: &str, r: &ScanReport) -> Result<()> {
    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json = serde_json::to_string_pretty(r).context("serializing scan report")?;
    fs::write(out_dir.join(format!("{basename}.json")), json).context("writing scan JSON")?;
    fs::write(out_dir.join(format!("{basename}.md")), render_scan_md(r))
        .context("writing scan MD")?;
    Ok(())
}

fn render_scan_md(r: &ScanReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} Report\n\n", r.tool));
    out.push_str(&format!(
        "Generated by `{} --mode {}` on {}.\n\n",
        r.tool, r.mode, r.today
    ));
    out.push_str("## Summary\n\n");
    out.push_str(&format!("- Workflows scanned: {}\n", r.summary.workflows));
    out.push_str(&format!("- Profiles declared: {}\n", r.summary.profiles));
    out.push_str(&format!(
        "- Unknown commands/endpoints total: {}\n",
        r.summary.unknown_total
    ));
    out.push_str(&format!(
        "- Profiles with missing fields: {}\n",
        r.summary.missing_fields
    ));
    out.push_str(&format!(
        "- Invalid profile findings: {}\n",
        r.summary.invalid_profiles
    ));
    out.push_str(&format!("- Stale profiles: {}\n", r.summary.stale));
    out.push_str(&format!("- Orphan profiles: {}\n\n", r.summary.orphaned));

    out.push_str("## Profile lifecycle findings\n\n");
    if r.findings.missing_fields.is_empty()
        && r.findings.invalid_profiles.is_empty()
        && r.findings.stale.is_empty()
        && r.findings.orphaned.is_empty()
    {
        out.push_str("_(none)_\n\n");
    } else {
        for finding in &r.findings.missing_fields {
            out.push_str(&format!(
                "- **Missing metadata** `{}`: {}\n",
                finding.profile,
                finding.missing.join(", ")
            ));
        }
        for finding in &r.findings.invalid_profiles {
            out.push_str(&format!(
                "- **Invalid profile** `{}` field `{}` value `{}`: {}\n",
                finding.profile, finding.field, finding.value, finding.reason
            ));
        }
        for finding in &r.findings.stale {
            out.push_str(&format!(
                "- **Stale profile** `{}`: review_after `{}` is before `{}`\n",
                finding.profile, finding.review_after, finding.today
            ));
        }
        for profile in &r.findings.orphaned {
            out.push_str(&format!("- **Orphan profile** `{profile}`\n"));
        }
        out.push('\n');
    }
    out.push_str("## Per-workflow\n\n");
    for w in &r.workflows {
        out.push_str(&format!(
            "### `{}` (profile: `{}`)\n\n",
            w.workflow, w.declared_profile
        ));
        out.push_str(&format!("- Detected: {}\n", join_or_none(&w.detected)));
        if w.unknown.is_empty() {
            out.push_str("- Unknown: _(none)_\n\n");
        } else {
            out.push_str(&format!("- **Unknown**: {}\n\n", w.unknown.join(", ")));
        }
    }
    out
}

fn join_or_none(v: &[String]) -> String {
    if v.is_empty() {
        "_(none)_".to_string()
    } else {
        v.join(", ")
    }
}

fn list_strings(out: &mut String, title: &str, items: &[String]) {
    out.push_str(&format!("## {} ({})\n\n", title, items.len()));
    if items.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for s in items {
            out.push_str(&format!("- `{s}`\n"));
        }
        out.push('\n');
    }
}

fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Advisory => "advisory",
        Mode::BlockingAllowlist => "blocking-allowlist",
        Mode::BlockingStrict => "blocking-strict",
    }
}

fn date_is_past(date: &str, today: &str) -> bool {
    let parsed = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok();
    let today_parsed = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok();
    match (parsed, today_parsed) {
        (Some(d), Some(t)) => d < t,
        _ => date.trim() < today,
    }
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set; run via `cargo xtask`")?;
    let xtask_dir = PathBuf::from(manifest_dir);
    let root = xtask_dir
        .parent()
        .with_context(|| format!("xtask manifest dir has no parent: {}", xtask_dir.display()))?
        .to_path_buf();
    Ok(root)
}

fn today_iso() -> String {
    chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

fn today_date() -> Result<NaiveDate> {
    let today = today_iso();
    NaiveDate::parse_from_str(&today, "%Y-%m-%d")
        .with_context(|| format!("parsing generated current date `{today}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_guard_scanner_accepts_guarded_jobs() {
        let yaml = r#"
name: Release

jobs:
  publish:
    if: github.repository == 'EffortlessMetrics/shipper' && github.event_name == 'push'
    runs-on: ubuntu-latest
    steps:
      - run: cargo publish
  rehearse:
    if: github.repository == "EffortlessMetrics/shipper" && github.event_name == 'workflow_dispatch'
    runs-on: ubuntu-latest
    steps:
      - run: cargo xtask policy-report
"#;

        let missing = workflow_jobs_missing_repository_guard(yaml, "EffortlessMetrics/shipper");

        assert!(missing.is_empty());
    }

    #[test]
    fn repository_guard_scanner_reports_unguarded_jobs() {
        let yaml = r#"
name: Release

jobs:
  publish:
    if: github.event_name == 'push'
    runs-on: ubuntu-latest
    steps:
      - run: cargo publish
  create-release:
    runs-on: ubuntu-latest
    steps:
      - run: gh release create
  rehearse:
    if: github.repository == 'EffortlessMetrics/shipper' && github.event_name == 'workflow_dispatch'
    runs-on: ubuntu-latest
    steps:
      - run: cargo xtask policy-report
"#;

        let missing = workflow_jobs_missing_repository_guard(yaml, "EffortlessMetrics/shipper");

        assert_eq!(missing, vec!["publish", "create-release"]);
    }

    #[test]
    fn repository_guard_scanner_ignores_inline_comment_bypass() {
        let yaml = r#"
name: Release

jobs:
  publish:
    if: github.repository == 'EffortlessMetrics/shipper-swarm' # github.repository == 'EffortlessMetrics/shipper'
    runs-on: ubuntu-latest
    steps:
      - run: cargo publish
  rehearse:
    if: github.repository == 'EffortlessMetrics/shipper'
    runs-on: ubuntu-latest
    steps:
      - run: cargo xtask policy-report
"#;

        let missing = workflow_jobs_missing_repository_guard(yaml, "EffortlessMetrics/shipper");

        assert_eq!(missing, vec!["publish"]);
    }

    #[test]
    fn repository_guard_scanner_accepts_multiline_job_if() {
        let yaml = r#"
name: Release

jobs:
  publish:
    if: >
      github.repository == 'EffortlessMetrics/shipper'
      && github.event_name == 'push'
    runs-on: ubuntu-latest
    steps:
      - run: cargo publish
"#;

        let missing = workflow_jobs_missing_repository_guard(yaml, "EffortlessMetrics/shipper");

        assert!(missing.is_empty());
    }

    #[test]
    fn repository_guard_scanner_ignores_multiline_comment_bypass() {
        let yaml = r#"
name: Release

jobs:
  publish:
    if: >
      github.repository == 'EffortlessMetrics/shipper-swarm'
      # github.repository == 'EffortlessMetrics/shipper'
    runs-on: ubuntu-latest
    steps:
      - run: cargo publish
"#;

        let missing = workflow_jobs_missing_repository_guard(yaml, "EffortlessMetrics/shipper");

        assert_eq!(missing, vec!["publish"]);
    }

    #[test]
    fn repository_guard_scanner_accepts_plain_multiline_job_if() {
        let yaml = r#"
name: Release

jobs:
  publish:
    if: github.event_name == 'push'
      && github.repository == 'EffortlessMetrics/shipper'
    runs-on: ubuntu-latest
    steps:
      - run: cargo publish
"#;

        let missing = workflow_jobs_missing_repository_guard(yaml, "EffortlessMetrics/shipper");

        assert!(missing.is_empty());
    }

    #[test]
    fn repository_guard_scanner_ignores_step_level_if() {
        let yaml = r#"
name: Release

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - if: github.repository == 'EffortlessMetrics/shipper'
        run: cargo publish
"#;

        let missing = workflow_jobs_missing_repository_guard(yaml, "EffortlessMetrics/shipper");

        assert_eq!(missing, vec!["publish"]);
    }

    #[test]
    fn authority_detector_rejects_temporary_self_mutating_workflow() {
        let yaml = r#"
name: Proof Pulse Repair

on:
  push:
    branches: ["repair/one-off"]

jobs:
  repair:
    permissions:
      contents: write
    steps:
      - name: Commit repair
        run: |
          git add .github/workflows/_temp-fix.yml
          git commit -m repair
          git push origin repair/one-off
      - name: Delete itself
        run: rm .github/workflows/_temp-fix.yml
"#;

        let findings = analyze_workflow_authority(
            ".github/workflows/_temp-fix.yml",
            "maintenance",
            None,
            yaml,
        );

        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "temporary-workflow-identity")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "temporary-branch-filter:repair")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "git-add")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "git-commit")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "git-push")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "workflow-self-deletion")
        );
    }

    #[test]
    fn authority_detector_catches_nested_shell_mutations() {
        let yaml = r#"
name: Temporary shell repair

on:
  workflow_dispatch:

jobs:
  repair:
    steps:
      - name: Nested mutation
        run: |
          echo $(git push origin main)
          echo `git commit -m repair`
          case "$MODE" in ready) git add .github/workflows/release.yml;; esac
"#;

        let findings =
            analyze_workflow_authority(".github/workflows/repair.yml", "maintenance", None, yaml);

        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "git-push")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "git-commit")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "git-add")
        );
    }

    #[test]
    fn authority_detector_does_not_flag_durable_repair_wording() {
        let yaml = r#"
name: Dependency Repair Guide

on:
  workflow_dispatch:

jobs:
  guide:
    steps:
      - run: echo "documented repair guidance"
"#;

        let findings = analyze_workflow_authority(
            ".github/workflows/dependency-repair-guide.yml",
            "maintenance",
            None,
            yaml,
        );

        assert!(findings.is_empty(), "unexpected findings: {findings:?}");

        let durable_path = analyze_workflow_authority(
            ".github/workflows/repair-rotation.yml",
            "Repair Rotation",
            None,
            yaml,
        );
        assert!(
            durable_path.is_empty(),
            "durable repair workflow was misclassified: {durable_path:?}"
        );
    }

    #[test]
    fn authority_detector_scopes_release_permissions_to_each_job() {
        let yaml = r#"
name: Release

on:
  workflow_dispatch:

jobs:
  publish:
    if: github.repository == 'EffortlessMetrics/shipper'
    permissions:
      id-token: write
    steps:
      - run: cargo publish
  inspect:
    permissions:
      id-token: write
    steps:
      - run: echo report
"#;

        let findings = analyze_workflow_authority(
            ".github/workflows/release.yml",
            "release",
            Some("EffortlessMetrics/shipper"),
            yaml,
        );

        assert!(
            !findings.iter().any(|finding| {
                finding.job == "publish" && finding.capability == "id-token:write"
            })
        );
        assert!(
            findings.iter().any(|finding| {
                finding.job == "inspect" && finding.capability == "id-token:write"
            })
        );
        assert!(findings.iter().any(|finding| {
            finding.job == "inspect"
                && finding.capability == "release-authority-without-repository-guard"
        }));
    }

    #[test]
    fn authority_detector_does_not_treat_nested_echo_as_mutation() {
        let yaml = r#"
name: Shell report

on:
  workflow_dispatch:

jobs:
  report:
    steps:
      - run: echo $(echo git add .)
"#;

        let findings = analyze_workflow_authority(
            ".github/workflows/_template.yml",
            "maintenance",
            None,
            yaml,
        );

        assert!(
            !findings
                .iter()
                .any(|finding| finding.capability == "git-add")
        );
        assert!(
            !findings
                .iter()
                .any(|finding| finding.capability == "temporary-workflow-identity")
        );
    }

    #[test]
    fn authority_detector_rejects_workflow_level_write_permissions() {
        let yaml = r#"
name: Mixed CI

on:
  push:
    branches: [main]

permissions:
  contents: write
  id-token: write
  pull-requests: write

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#;

        let findings = analyze_workflow_authority(".github/workflows/ci.yml", "ci", None, yaml);

        assert!(findings.iter().any(|finding| {
            finding.job == "<workflow>" && finding.capability == "contents:write"
        }));
        assert!(findings.iter().any(|finding| {
            finding.job == "<workflow>" && finding.capability == "id-token:write"
        }));
        assert!(findings.iter().any(|finding| {
            finding.job == "<workflow>" && finding.capability == "pull-requests:write"
        }));
    }

    #[test]
    fn authority_detector_accepts_guarded_release_job_local_permissions() {
        let yaml = r#"
name: Release

on:
  push:
    tags: ["v*.*.*"]
  workflow_dispatch:
    inputs:
      ref:
        description: Exact approved SHA to check out.

permissions:
  contents: read

jobs:
  identity:
    if: github.repository == 'EffortlessMetrics/shipper'
    permissions:
      contents: read
    steps:
      - name: Validate release identity
        run: cargo xtask release-identity --approved_sha "$SHA"
  publish:
    if: github.repository == 'EffortlessMetrics/shipper'
    permissions:
      contents: read
      id-token: write
    steps:
      - name: Publish
        run: cargo publish
"#;

        let findings = analyze_workflow_authority(
            ".github/workflows/release.yml",
            "release",
            Some("EffortlessMetrics/shipper"),
            yaml,
        );

        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
    }

    #[test]
    fn authority_detector_rejects_untrusted_pull_request_target_execution() {
        let yaml = r#"
name: Unsafe target workflow

on:
  pull_request_target:
    types: [opened]

jobs:
  test:
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - name: Run PR code
        env:
          SECRET: ${{ secrets.SECRET }}
        run: cargo test
"#;

        let findings = analyze_workflow_authority(".github/workflows/unsafe.yml", "ci", None, yaml);

        assert!(
            findings
                .iter()
                .any(|finding| { finding.capability == "pull-request-target-untrusted-execution" })
        );

        let workflow_level = r#"
name: Unsafe target workflow

on:
  pull_request_target:
    types: [opened]

permissions:
  contents: write

jobs:
  test:
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - name: Run PR code
        run: cargo test
"#;
        let workflow_level_findings =
            analyze_workflow_authority(".github/workflows/unsafe.yml", "ci", None, workflow_level);
        assert!(
            workflow_level_findings
                .iter()
                .any(|finding| { finding.capability == "pull-request-target-untrusted-execution" })
        );
    }

    #[test]
    fn authority_detector_rejects_label_cancellation_and_mutable_release_ref() {
        let labeled = r#"
name: Label gate

on:
  pull_request:
    types: [opened, labeled]

concurrency:
  group: code-${{ github.ref }}
  cancel-in-progress: true

jobs:
  check:
    steps:
      - run: cargo test
"#;
        let labeled_findings =
            analyze_workflow_authority(".github/workflows/label.yml", "ci", None, labeled);
        assert!(
            labeled_findings
                .iter()
                .any(|finding| { finding.capability == "label-triggered-cancellation" })
        );
        let unlabeled = labeled.replace("labeled", "unlabeled");
        let unlabeled_findings =
            analyze_workflow_authority(".github/workflows/unlabeled.yml", "ci", None, &unlabeled);
        assert!(
            !unlabeled_findings
                .iter()
                .any(|finding| finding.capability == "label-triggered-cancellation")
        );

        let release = r#"
name: Release

on:
  push:
    tags: ["v*.*.*"]
  workflow_dispatch:
    inputs:
      ref:
        default: main

jobs:
  publish:
    if: github.repository == 'EffortlessMetrics/shipper'
    permissions:
      id-token: write
    steps:
      - run: cargo publish
"#;
        let release_findings = analyze_workflow_authority(
            ".github/workflows/release.yml",
            "release",
            Some("EffortlessMetrics/shipper"),
            release,
        );
        assert!(
            release_findings
                .iter()
                .any(|finding| { finding.capability == "mutable-release-dispatch-ref" })
        );
        assert!(
            release_findings.iter().any(|finding| {
                finding.capability == "tag-release-without-approved-source-gate"
            })
        );
    }

    #[test]
    fn authority_detector_handles_block_branch_filters_and_dispatch_scope() {
        let yaml = r#"
name: Repair gate

on:
  push:
    branches:
      - repair/one-off
  workflow_dispatch:
    inputs:
      target:
        description: "A ref-like target, not the release ref input"
      ref:
        description: Exact approved SHA.
"#;

        let findings =
            analyze_workflow_authority(".github/workflows/repair.yml", "maintenance", None, yaml);

        assert!(
            findings
                .iter()
                .any(|finding| { finding.capability == "temporary-branch-filter:repair" })
        );
        assert!(has_dispatch_ref_input(yaml));
        assert!(!has_dispatch_ref_input(
            "on:\n  workflow_dispatch:\n    inputs:\n      target:\n        default: main\nenv:\n  ref: unrelated\n"
        ));
    }

    #[test]
    fn authority_detector_bounds_each_gh_api_invocation() {
        let yaml = r#"
name: Release inspection

on:
  workflow_dispatch:

jobs:
  inspect:
    steps:
      - name: Read and mutate separate refs
        run: |
          gh api repos/EffortlessMetrics/shipper/git/ref/heads/main --method get
          gh api repos/EffortlessMetrics/shipper/git/ref/heads/main --method POST
          gh api repos/EffortlessMetrics/shipper/git/ref/heads/main --method get
          curl -X POST https://example.invalid/release
"#;

        let findings =
            analyze_workflow_authority(".github/workflows/inspect.yml", "ci", None, yaml);

        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "github-api-mutation")
        );
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.capability == "github-api-mutation")
                .count(),
            1
        );
    }

    #[test]
    fn authority_detector_only_flags_mutating_git_tag_forms() {
        assert!(self_mutation_capabilities("git tag release").contains(&"git-tag-mutation"));
        assert!(!self_mutation_capabilities("git tag").contains(&"git-tag-mutation"));
        assert!(!self_mutation_capabilities("git tag -l").contains(&"git-tag-mutation"));
        assert!(!self_mutation_capabilities("git tag --list").contains(&"git-tag-mutation"));
        assert!(!self_mutation_capabilities("git tag -n").contains(&"git-tag-mutation"));
    }

    #[test]
    fn read_only_git_tag_listing_options_are_not_mutations() {
        for read_only in [
            "git tag -n5",
            "git tag --sort=-v:refname",
            "git tag --contains HEAD",
            "git tag --points-at HEAD",
            "git tag --merged",
            "git tag --no-merged main",
            "git tag --format='%(refname:short)'",
            "git tag -l --sort=creatordate",
            "git tag -i --list 'v*'",
        ] {
            assert!(
                !self_mutation_capabilities(read_only).contains(&"git-tag-mutation"),
                "{read_only} reads refs and must not be reported as mutation"
            );
        }

        for mutating in [
            "git tag -a v1.0.0 -m release",
            "git tag -d v1.0.0",
            "git tag --delete v1.0.0",
            "git tag -f v1.0.0",
            "git tag -s v1.0.0",
            "git tag v1.0.0",
            // A listing option first must not launder a delete later in the same command.
            "git tag --sort=creatordate -d v1.0.0",
            // An unrecognised option keeps the detector failing closed.
            "git tag --future-option v1.0.0",
        ] {
            assert!(
                self_mutation_capabilities(mutating).contains(&"git-tag-mutation"),
                "{mutating} mutates refs and must be reported"
            );
        }
    }

    #[test]
    fn backslash_continuations_do_not_hide_mutating_invocations() {
        let yaml = r#"
name: Continuation

on:
  workflow_dispatch:

jobs:
  mutate:
    steps:
      - name: Mutate across lines
        run: |
          gh api repos/EffortlessMetrics/shipper/git/refs \
            --method POST \
            --field ref=refs/heads/temp
"#;

        let findings = analyze_workflow_authority(".github/workflows/cont.yml", "ci", None, yaml);

        assert!(
            findings
                .iter()
                .any(|finding| finding.capability == "github-api-mutation"),
            "a backslash-continued gh api mutation must still be reported: {findings:?}"
        );

        assert!(
            self_mutation_capabilities("git \\\n  push origin main").contains(&"git-push"),
            "a backslash-continued git push must still be reported"
        );

        // Splicing must not glue independent commands together.
        assert!(
            !self_mutation_capabilities(
                "gh api repos/EffortlessMetrics/shipper/git/refs\ncurl -X POST https://example.invalid"
            )
            .contains(&"github-api-mutation"),
            "separate lines stay separate invocations"
        );
    }

    #[test]
    fn release_identity_language_requires_an_executable_command() {
        let prose_only = r#"
name: Release

on:
  workflow_dispatch:
    inputs:
      ref:
        description: "Use the release-identity gate and approved_sha."

jobs:
  publish:
    steps:
      - name: Release identity gate
        run: echo "exact approved SHA"
      - name: Publish
        run: cargo publish
"#;
        assert!(!has_exact_release_identity_language(prose_only));

        let executable = prose_only.replace(
            "run: echo \"exact approved SHA\"",
            "run: cargo xtask release-identity --approved-sha \"$SHA\"",
        );
        assert!(has_exact_release_identity_language(&executable));
    }

    #[test]
    fn authority_detector_models_scalar_permissions() {
        let write_all = r#"
name: Broad permissions

on: workflow_dispatch

permissions: write-all

jobs:
  check:
    steps:
      - run: cargo test
"#;
        let read_all = write_all.replace("write-all", "read-all");

        let write_findings =
            analyze_workflow_authority(".github/workflows/broad.yml", "ci", None, write_all);
        assert!(
            write_findings
                .iter()
                .any(|finding| { finding.job == "<workflow>" && finding.capability == "*:write" })
        );

        let read_findings =
            analyze_workflow_authority(".github/workflows/read.yml", "ci", None, &read_all);
        assert!(
            !read_findings
                .iter()
                .any(|finding| finding.capability == "*:read")
        );

        let quoted = write_all.replace("permissions: write-all", "permissions: \"write-all\"");
        let quoted_findings =
            analyze_workflow_authority(".github/workflows/quoted.yml", "ci", None, &quoted);
        assert!(
            quoted_findings
                .iter()
                .any(|finding| finding.capability == "*:write")
        );

        let unknown = write_all.replace("permissions: write-all", "permissions: writte-all");
        let unknown_findings =
            analyze_workflow_authority(".github/workflows/unknown.yml", "ci", None, &unknown);
        assert!(
            unknown_findings
                .iter()
                .any(|finding| { finding.capability == "unknown-permission-scalar:writte-all" })
        );

        let empty = write_all.replace("permissions: write-all", "permissions: {}");
        let empty_findings =
            analyze_workflow_authority(".github/workflows/empty.yml", "ci", None, &empty);
        assert!(
            !empty_findings
                .iter()
                .any(|finding| finding.capability.starts_with("unknown-permission-scalar:")),
            "valid empty permissions mapping was rejected: {empty_findings:?}"
        );
    }

    #[test]
    fn authority_detector_ignores_read_only_reconciliation_wording() {
        let yaml = r#"
name: Registry remediation report

on:
  workflow_dispatch:

permissions:
  contents: read

jobs:
  report:
    steps:
      - name: Explain remediation
        run: echo "repair guidance"
"#;

        let findings = analyze_workflow_authority(".github/workflows/report.yml", "ci", None, yaml);

        assert!(findings.is_empty(), "unexpected findings: {findings:?}");

        let actual_command = yaml.replace(
            "run: echo \"repair guidance\"",
            "run: cargo publish --dry-run",
        );
        let command_findings = analyze_workflow_authority(
            ".github/workflows/report.yml",
            "release",
            None,
            &actual_command,
        );
        assert!(
            command_findings.iter().any(|finding| {
                finding.capability == "release-authority-without-repository-guard"
            })
        );
    }

    #[test]
    fn authority_detector_does_not_misclassify_read_only_gh_api() {
        let yaml = r#"
name: Release

on:
  workflow_dispatch:

jobs:
  identity:
    if: github.repository == 'EffortlessMetrics/shipper'
    steps:
      - name: Read ref
        run: gh api repos/EffortlessMetrics/shipper/git/ref/heads/main
      - name: Explain the command
        run: echo "git push origin main"
"#;

        let findings = analyze_workflow_authority(
            ".github/workflows/release.yml",
            "release",
            Some("EffortlessMetrics/shipper"),
            yaml,
        );

        assert!(
            !findings
                .iter()
                .any(|finding| finding.capability == "github-api-mutation")
        );
        assert!(
            !findings
                .iter()
                .any(|finding| finding.capability == "git-push")
        );
    }

    // ─── Authority exception reconciliation ────────────────────────────────

    fn reconciliation_today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 8).expect("valid date")
    }

    fn live_finding() -> WorkflowAuthorityFinding {
        authority_finding(
            ".github/workflows/droid-security-scan.yml",
            "droid-security-scan",
            "<permissions>",
            "contents:write",
            "schedule,workflow_dispatch",
            "not declared",
            "move the grant to the narrow job that needs it and prove the repository boundary",
        )
    }

    fn matching_record() -> AuthorityException {
        AuthorityException {
            workflow: ".github/workflows/droid-security-scan.yml".to_string(),
            job: "droid-security-scan".to_string(),
            step: "<permissions>".to_string(),
            capability: "contents:write".to_string(),
            trigger: "schedule,workflow_dispatch".to_string(),
            repository: "EffortlessMetrics/shipper-swarm".to_string(),
            finding_repository_boundary: "not declared".to_string(),
            owner: "release/ci".to_string(),
            reason: "A durable security-report branch requires exact repository write authority."
                .to_string(),
            covered_by: "Scheduled/manual triggers, fixed action SHA, trusted runner, and review."
                .to_string(),
            created: "2026-08-06".to_string(),
            review_after: "2026-11-06".to_string(),
        }
    }

    /// Count only the reconciliation states the blocking modes reject.
    fn blocking_authority_count(out: &AuthorityReconciliation) -> usize {
        out.unexcepted.len() + out.expired.len() + out.drifted.len() + out.unused.len()
    }

    fn empty_findings() -> WorkflowFindings {
        WorkflowFindings {
            unreceipted: Vec::new(),
            missing_fields: Vec::new(),
            expired: Vec::new(),
            stale: Vec::new(),
            unused: Vec::new(),
            invalid_policy_refs: Vec::new(),
            repository_guard_violations: Vec::new(),
            authority_violations: Vec::new(),
            authorized_exceptions: Vec::new(),
            unexcepted_authority: Vec::new(),
            expired_exceptions: Vec::new(),
            drifted_exceptions: Vec::new(),
            unused_exceptions: Vec::new(),
            invalid_authority_ledger: Vec::new(),
        }
    }

    #[test]
    fn exact_record_authorizes_its_finding_without_blocking() {
        let out = reconcile_authority_exceptions(
            &[live_finding()],
            &[matching_record()],
            reconciliation_today(),
        );

        assert_eq!(out.authorized.len(), 1, "{out:#?}");
        assert_eq!(out.authorized[0].capability, "contents:write");
        assert_eq!(out.authorized[0].owner, "release/ci");
        assert!(out.unexcepted.is_empty(), "{out:#?}");
        assert!(out.expired.is_empty(), "{out:#?}");
        assert!(out.drifted.is_empty(), "{out:#?}");
        assert!(out.unused.is_empty(), "{out:#?}");
        assert_eq!(blocking_authority_count(&out), 0);
    }

    #[test]
    fn unexcepted_finding_blocks() {
        let out = reconcile_authority_exceptions(&[live_finding()], &[], reconciliation_today());

        assert_eq!(out.unexcepted, vec![live_finding()], "{out:#?}");
        assert!(out.authorized.is_empty(), "{out:#?}");
        assert_eq!(blocking_authority_count(&out), 1);
    }

    #[test]
    fn expired_record_blocks_and_does_not_authorize() {
        let mut record = matching_record();
        record.review_after = "2026-08-07".to_string();

        let out =
            reconcile_authority_exceptions(&[live_finding()], &[record], reconciliation_today());

        assert!(out.authorized.is_empty(), "{out:#?}");
        assert_eq!(out.expired.len(), 1, "{out:#?}");
        assert_eq!(out.expired[0].review_after, "2026-08-07");
        assert_eq!(out.expired[0].today, "2026-08-08");
        assert!(out.unused.is_empty(), "{out:#?}");
        assert_eq!(blocking_authority_count(&out), 1);
    }

    #[test]
    fn unparseable_review_date_is_treated_as_expired() {
        let mut record = matching_record();
        record.review_after = "soon".to_string();

        let out =
            reconcile_authority_exceptions(&[live_finding()], &[record], reconciliation_today());

        assert!(out.authorized.is_empty(), "{out:#?}");
        assert_eq!(out.expired.len(), 1, "{out:#?}");
    }

    #[test]
    fn trigger_drift_blocks_and_authorizes_nothing() {
        let mut record = matching_record();
        record.trigger = "workflow_dispatch".to_string();

        let out =
            reconcile_authority_exceptions(&[live_finding()], &[record], reconciliation_today());

        assert!(out.authorized.is_empty(), "{out:#?}");
        assert_eq!(out.drifted.len(), 1, "{out:#?}");
        assert_eq!(
            out.drifted[0].drifted,
            vec![DriftedField {
                field: "trigger",
                expected: "workflow_dispatch".to_string(),
                actual: "schedule,workflow_dispatch".to_string(),
            }]
        );
        // The drifted record is consumed by the drift finding, so it is not
        // also double-reported as unused.
        assert!(out.unused.is_empty(), "{out:#?}");
        assert_eq!(blocking_authority_count(&out), 1);
    }

    #[test]
    fn repository_boundary_drift_blocks_and_authorizes_nothing() {
        let mut record = matching_record();
        record.finding_repository_boundary = "EffortlessMetrics/shipper".to_string();

        let out =
            reconcile_authority_exceptions(&[live_finding()], &[record], reconciliation_today());

        assert!(out.authorized.is_empty(), "{out:#?}");
        assert_eq!(out.drifted.len(), 1, "{out:#?}");
        assert_eq!(
            out.drifted[0].drifted,
            vec![DriftedField {
                field: "finding_repository_boundary",
                expected: "EffortlessMetrics/shipper".to_string(),
                actual: "not declared".to_string(),
            }]
        );
        assert_eq!(blocking_authority_count(&out), 1);
    }

    #[test]
    fn drift_names_every_drifted_field_with_both_values() {
        let mut record = matching_record();
        record.trigger = "push".to_string();
        record.finding_repository_boundary = "EffortlessMetrics/shipper".to_string();

        let out =
            reconcile_authority_exceptions(&[live_finding()], &[record], reconciliation_today());

        let fields: Vec<&str> = out.drifted[0]
            .drifted
            .iter()
            .map(|field| field.field)
            .collect();
        assert_eq!(fields, vec!["trigger", "finding_repository_boundary"]);
        assert_eq!(out.drifted[0].drifted[0].expected, "push");
        assert_eq!(
            out.drifted[0].drifted[0].actual,
            "schedule,workflow_dispatch"
        );
    }

    #[test]
    fn a_record_for_another_capability_never_absorbs_a_finding() {
        let mut record = matching_record();
        record.capability = "actions:write".to_string();

        let out =
            reconcile_authority_exceptions(&[live_finding()], &[record], reconciliation_today());

        assert!(out.drifted.is_empty(), "{out:#?}");
        assert_eq!(out.unexcepted, vec![live_finding()], "{out:#?}");
        assert_eq!(out.unused.len(), 1, "{out:#?}");
        assert_eq!(blocking_authority_count(&out), 2);
    }

    #[test]
    fn unused_record_blocks() {
        let out = reconcile_authority_exceptions(&[], &[matching_record()], reconciliation_today());

        assert_eq!(out.unused.len(), 1, "{out:#?}");
        assert!(
            out.unused[0]
                .reason
                .contains("no detector authority finding"),
            "{out:#?}"
        );
        assert_eq!(blocking_authority_count(&out), 1);
    }

    #[test]
    fn a_record_may_not_authorize_an_unknown_permission_scalar() {
        let finding = authority_finding(
            ".github/workflows/droid-security-scan.yml",
            "droid-security-scan",
            "<permissions>",
            "unknown-permission-scalar:inherit",
            "schedule,workflow_dispatch",
            "not declared",
            "move the grant to the narrow job that needs it and prove the repository boundary",
        );
        let mut record = matching_record();
        record.capability = "unknown-permission-scalar:inherit".to_string();

        let out = reconcile_authority_exceptions(
            std::slice::from_ref(&finding),
            &[record],
            reconciliation_today(),
        );

        assert!(out.authorized.is_empty(), "{out:#?}");
        assert!(out.drifted.is_empty(), "{out:#?}");
        assert_eq!(out.unexcepted, vec![finding], "{out:#?}");
        assert_eq!(out.unused.len(), 1, "{out:#?}");
        assert!(
            out.unused[0].reason.contains("unparsed authority shape"),
            "{out:#?}"
        );
        // The unexcepted finding and the rejected record each block.
        assert_eq!(blocking_authority_count(&out), 2);
    }

    #[test]
    fn every_unexcepted_or_invalid_authority_state_blocks() {
        for mode in [Mode::BlockingAllowlist, Mode::BlockingStrict] {
            let mut findings = empty_findings();
            findings.unexcepted_authority = vec![live_finding()];
            assert_eq!(workflow_blocking_count(mode, &findings), 1, "{mode:?}");

            let mut findings = empty_findings();
            findings.invalid_authority_ledger = vec!["ledger will not parse".to_string()];
            assert_eq!(workflow_blocking_count(mode, &findings), 1, "{mode:?}");

            let mut findings = empty_findings();
            findings.expired_exceptions = vec![ExpiredException {
                workflow: ".github/workflows/droid-security-scan.yml".to_string(),
                job: "droid-security-scan".to_string(),
                step: "<permissions>".to_string(),
                capability: "contents:write".to_string(),
                trigger: "schedule,workflow_dispatch".to_string(),
                repository_boundary: "not declared".to_string(),
                owner: "release/ci".to_string(),
                review_after: "2026-08-07".to_string(),
                today: "2026-08-08".to_string(),
            }];
            assert_eq!(workflow_blocking_count(mode, &findings), 1, "{mode:?}");

            let mut findings = empty_findings();
            findings.drifted_exceptions = vec![DriftedException {
                workflow: ".github/workflows/droid-security-scan.yml".to_string(),
                job: "droid-security-scan".to_string(),
                step: "<permissions>".to_string(),
                capability: "contents:write".to_string(),
                owner: "release/ci".to_string(),
                drifted: vec![DriftedField {
                    field: "trigger",
                    expected: "workflow_dispatch".to_string(),
                    actual: "schedule,workflow_dispatch".to_string(),
                }],
                remediation: "re-review".to_string(),
            }];
            assert_eq!(workflow_blocking_count(mode, &findings), 1, "{mode:?}");

            let mut findings = empty_findings();
            findings.unused_exceptions = vec![UnusedException {
                workflow: ".github/workflows/droid-security-scan.yml".to_string(),
                job: "droid-security-scan".to_string(),
                step: "<permissions>".to_string(),
                capability: "contents:write".to_string(),
                trigger: "schedule,workflow_dispatch".to_string(),
                finding_repository_boundary: "not declared".to_string(),
                owner: "release/ci".to_string(),
                review_after: "2026-11-06".to_string(),
                reason: "unused".to_string(),
            }];
            assert_eq!(workflow_blocking_count(mode, &findings), 1, "{mode:?}");
        }
    }

    #[test]
    fn an_authorized_exception_is_never_blocking_but_stays_visible() {
        let mut findings = empty_findings();
        findings.authority_violations = vec![live_finding()];
        findings.authorized_exceptions = vec![AuthorizedException {
            workflow: ".github/workflows/droid-security-scan.yml".to_string(),
            job: "droid-security-scan".to_string(),
            step: "<permissions>".to_string(),
            capability: "contents:write".to_string(),
            trigger: "schedule,workflow_dispatch".to_string(),
            repository_boundary: "not declared".to_string(),
            repository: "EffortlessMetrics/shipper-swarm".to_string(),
            owner: "release/ci".to_string(),
            review_after: "2026-11-06".to_string(),
        }];

        assert_eq!(
            workflow_blocking_count(Mode::BlockingAllowlist, &findings),
            0
        );
        assert_eq!(workflow_blocking_count(Mode::BlockingStrict, &findings), 0);
        // The raw detector total keeps the accepted capability visible.
        assert_eq!(findings.authority_violations.len(), 1);
    }

    #[test]
    fn an_invalid_ledger_is_reported_rather_than_panicking() {
        // `xtask` declares no dev-dependencies and its manifest is covered by the
        // dependency-surface policy, so `serial_test`/`tempfile` are not
        // available here. Give each temp directory a process- and
        // invocation-unique name instead, which removes the collision this
        // isolation would have prevented.
        let temp = unique_temp_dir("shipper-authority-ledger");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(temp.join("policy")).expect("temp policy dir");
        fs::write(
            temp.join(authority_exceptions::LEDGER),
            "schema_version = \"9.9\"\npolicy = \"workflow-authority-exceptions\"\nowner = \"release/ci\"\nstatus = \"active\"\n",
        )
        .expect("write ledger");

        let (records, invalid) = load_authority_records(&temp, NaiveDate::from_ymd_opt(2026, 8, 8));

        assert!(records.is_empty(), "{records:#?}");
        assert_eq!(invalid.len(), 1, "{invalid:#?}");
        assert!(invalid[0].contains("schema"), "{invalid:#?}");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn a_record_that_fails_validation_never_reports_as_authorized() {
        // The record below matches the live finding on all six identity fields
        // but names a repository outside the organization, so the validator
        // rejects it. Before this was fixed the reconciler still matched it and
        // the report printed ACCEPTED for a record the validator had refused —
        // and in advisory mode that report is the only output.
        let temp = unique_temp_dir("shipper-authority-invalid-authorizes");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(temp.join(".github/workflows")).expect("temp workflow dir");
        fs::write(temp.join(".github/workflows/scan.yml"), "name: Scan\n").expect("write workflow");
        fs::create_dir_all(temp.join("policy")).expect("temp policy dir");
        fs::write(
            temp.join(authority_exceptions::LEDGER),
            r#"schema_version = "1.0"
policy = "workflow-authority-exceptions"
owner = "release/ci"
status = "active"

[[authority_exception]]
workflow = ".github/workflows/scan.yml"
job = "scan"
step = "<permissions>"
capability = "contents:write"
trigger = "schedule"
repository = "Attacker/evil"
finding_repository_boundary = "not declared"
owner = "release/ci"
reason = "A durable security-report branch requires exact repository write authority."
covered_by = "Scheduled triggers, fixed action SHA, trusted runner, and review."
created = "2026-01-01"
review_after = "2026-11-06"
"#,
        )
        .expect("write ledger");

        let (records, invalid) = load_authority_records(&temp, NaiveDate::from_ymd_opt(2026, 8, 8));

        assert!(
            records.is_empty(),
            "an invalid ledger must yield no authorization candidates: {records:#?}"
        );
        assert_eq!(invalid.len(), 1, "{invalid:#?}");
        assert!(invalid[0].contains("EffortlessMetrics"), "{invalid:#?}");

        // And the finding it aimed at reports unexcepted, not authorized.
        let finding = WorkflowAuthorityFinding {
            workflow: ".github/workflows/scan.yml".to_string(),
            job: "scan".to_string(),
            step: "<permissions>".to_string(),
            capability: "contents:write".to_string(),
            trigger: "schedule".to_string(),
            repository_boundary: "not declared".to_string(),
            remediation: "remove the grant".to_string(),
        };
        let reconciled = reconcile_authority_exceptions(
            std::slice::from_ref(&finding),
            &records,
            NaiveDate::from_ymd_opt(2026, 8, 8).expect("date"),
        );
        assert!(reconciled.authorized.is_empty(), "{reconciled:#?}");
        assert_eq!(reconciled.unexcepted.len(), 1, "{reconciled:#?}");

        let _ = fs::remove_dir_all(&temp);
    }

    fn valid_profile(name: &str, review_after: &str) -> RawProfile {
        RawProfile {
            name: Some(name.to_string()),
            allowed_processes: vec!["cargo".to_string()],
            allowed_endpoints: vec!["crates.io".to_string()],
            owner: Some("release/ci".to_string()),
            reason: Some("Required by the test workflow.".to_string()),
            created: Some("2026-01-01".to_string()),
            review_after: Some(review_after.to_string()),
        }
    }

    #[test]
    fn profile_lifecycle_accepts_current_referenced_profiles() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 12).expect("valid fixture date");
        let referenced = BTreeSet::from(["ci".to_string(), "release".to_string()]);
        let catalog = reconcile_profiles(
            vec![
                valid_profile("ci", "2026-08-12"),
                valid_profile("release", "2026-08-13"),
            ],
            &referenced,
            today,
        );

        assert_eq!(catalog.entries, 2);
        assert_eq!(catalog.profiles.len(), 2);
        assert!(catalog.findings.missing_fields.is_empty());
        assert!(catalog.findings.invalid_profiles.is_empty());
        assert!(catalog.findings.stale.is_empty());
        assert!(catalog.findings.orphaned.is_empty());
    }

    #[test]
    fn profile_lifecycle_classifies_blank_and_invalid_fields_once() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 12).expect("valid fixture date");
        let mut blank_owner = valid_profile("blank-owner", "2026-08-13");
        blank_owner.owner = Some("  ".to_string());
        let mut malformed_created = valid_profile("bad-created", "2026-08-13");
        malformed_created.created = Some("2026-8-01".to_string());
        let mut impossible_review = valid_profile("bad-review", "2026-02-30");
        impossible_review.created = Some("2026-01-01".to_string());
        let mut reversed = valid_profile("reversed", "2026-01-01");
        reversed.created = Some("2026-01-02".to_string());
        let mut blank_name = valid_profile("placeholder", "2026-08-13");
        blank_name.name = Some("  ".to_string());

        let catalog = reconcile_profiles(
            vec![
                blank_owner,
                malformed_created,
                impossible_review,
                reversed,
                blank_name,
            ],
            &BTreeSet::new(),
            today,
        );

        assert_eq!(catalog.findings.missing_fields.len(), 2);
        assert_eq!(catalog.findings.missing_fields[0].profile, "blank-owner");
        assert_eq!(catalog.findings.missing_fields[0].missing, ["owner"]);
        assert_eq!(catalog.findings.missing_fields[1].profile, "profile[4]");
        assert_eq!(catalog.findings.missing_fields[1].missing, ["name"]);
        assert_eq!(catalog.findings.invalid_profiles.len(), 3);
        assert!(catalog.findings.stale.is_empty());
        assert!(catalog.findings.orphaned.is_empty());
        assert!(catalog.profiles.is_empty());
    }

    #[test]
    fn duplicate_profile_names_fail_closed_without_last_write_wins() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 12).expect("valid fixture date");
        let mut first = valid_profile("ci", "2026-08-13");
        first.allowed_processes = vec!["cargo".to_string()];
        let mut second = valid_profile("ci", "2026-08-13");
        second.allowed_processes = vec!["curl".to_string()];

        let catalog = reconcile_profiles(
            vec![first, second],
            &BTreeSet::from(["ci".to_string()]),
            today,
        );

        assert_eq!(catalog.findings.invalid_profiles.len(), 1);
        assert!(
            catalog.findings.invalid_profiles[0]
                .reason
                .contains("appears 2 times")
        );
        assert!(!catalog.profiles.contains_key("ci"));
    }

    #[test]
    fn stale_and_orphan_findings_only_block_strict_mode() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 12).expect("valid fixture date");
        let catalog = reconcile_profiles(
            vec![
                valid_profile("stale", "2026-08-11"),
                valid_profile("orphan", "2026-08-12"),
            ],
            &BTreeSet::from(["stale".to_string()]),
            today,
        );

        assert_eq!(catalog.findings.stale.len(), 1);
        assert_eq!(catalog.findings.orphaned, ["orphan"]);
        assert_eq!(
            profile_blocking_count(Mode::Advisory, &catalog.findings, 0),
            0
        );
        assert_eq!(
            profile_blocking_count(Mode::BlockingAllowlist, &catalog.findings, 0),
            0
        );
        assert_eq!(
            profile_blocking_count(Mode::BlockingStrict, &catalog.findings, 0),
            2
        );
    }

    #[test]
    fn missing_invalid_and_unknown_findings_block_both_blocking_modes() {
        let findings = ProfileLifecycleFindings {
            missing_fields: vec![ProfileMissingFields {
                profile: "ci".to_string(),
                missing: vec!["owner".to_string()],
            }],
            invalid_profiles: vec![InvalidProfile {
                profile: "release".to_string(),
                field: "created",
                value: "soon".to_string(),
                reason: "invalid date".to_string(),
            }],
            ..ProfileLifecycleFindings::default()
        };

        assert_eq!(
            profile_blocking_count(Mode::BlockingAllowlist, &findings, 1),
            3
        );
        assert_eq!(
            profile_blocking_count(Mode::BlockingStrict, &findings, 1),
            3
        );
    }

    #[test]
    fn profile_references_use_only_live_non_dependabot_workflows() {
        let entries = vec![
            RawWorkflowEntry {
                path: Some(".github/workflows/live.yml".to_string()),
                kind: Some("workflow".to_string()),
                owner: None,
                reason: None,
                process_policy: Some("ci".to_string()),
                network_policy: Some("network-ci".to_string()),
                required_repository_guard: None,
                created: None,
                review_after: None,
                expires: None,
            },
            RawWorkflowEntry {
                path: Some(".github/workflows/removed.yml".to_string()),
                kind: Some("workflow".to_string()),
                owner: None,
                reason: None,
                process_policy: Some("stale-receipt".to_string()),
                network_policy: Some("stale-network".to_string()),
                required_repository_guard: None,
                created: None,
                review_after: None,
                expires: None,
            },
            RawWorkflowEntry {
                path: Some(".github/dependabot.yml".to_string()),
                kind: Some("dependabot_config".to_string()),
                owner: None,
                reason: None,
                process_policy: Some("dependabot".to_string()),
                network_policy: Some("dependabot".to_string()),
                required_repository_guard: None,
                created: None,
                review_after: None,
                expires: None,
            },
        ];
        let workflows = vec![".github/workflows/live.yml".to_string()];

        assert_eq!(
            referenced_profiles(&entries, &workflows, ProfileKind::Process),
            BTreeSet::from(["ci".to_string()])
        );
        assert_eq!(
            referenced_profiles(&entries, &workflows, ProfileKind::Network),
            BTreeSet::from(["network-ci".to_string()])
        );
    }

    #[test]
    fn scan_markdown_and_json_keep_lifecycle_findings_structured() {
        let report = ScanReport {
            tool: "cargo xtask check-process-policy",
            mode: "advisory",
            today: "2026-08-12".to_string(),
            summary: ScanSummary {
                workflows: 1,
                profiles: 2,
                unknown_total: 0,
                missing_fields: 0,
                invalid_profiles: 0,
                stale: 1,
                orphaned: 1,
            },
            findings: ProfileLifecycleFindings {
                stale: vec![StaleProfile {
                    profile: "ci".to_string(),
                    review_after: "2026-08-11".to_string(),
                    today: "2026-08-12".to_string(),
                }],
                orphaned: vec!["unused".to_string()],
                ..ProfileLifecycleFindings::default()
            },
            workflows: Vec::new(),
        };

        let json = serde_json::to_value(&report).expect("serialize report");
        assert_eq!(json["summary"]["stale"], 1);
        assert_eq!(json["findings"]["orphaned"][0], "unused");
        let markdown = render_scan_md(&report);
        assert!(markdown.contains("**Stale profile** `ci`"));
        assert!(markdown.contains("**Orphan profile** `unused`"));
    }

    #[test]
    fn process_and_network_adapters_enforce_lifecycle_modes_and_reports() {
        #[derive(Clone, Copy)]
        struct Case {
            name: &'static str,
            profiles: fn(&str, &str) -> String,
            missing: usize,
            invalid: usize,
            stale: usize,
            orphaned: usize,
            allowlist_blocks: bool,
            strict_blocks: bool,
        }

        fn profile(name: &str, created: &str, review_after: &str) -> String {
            format!(
                r#"[[profile]]
name = "{name}"
allowed_processes = ["cargo", "curl"]
allowed_endpoints = ["crates.io"]
owner = "release/ci"
reason = "Required by the live fixture workflow."
created = "{created}"
review_after = "{review_after}"
"#
            )
        }

        let today = today_iso();
        let yesterday = NaiveDate::parse_from_str(&today, "%Y-%m-%d")
            .expect("current date parses")
            .pred_opt()
            .expect("current date has a predecessor")
            .format("%Y-%m-%d")
            .to_string();
        let cases = [
            Case {
                name: "valid",
                profiles: |today, _| profile("ci", today, today),
                missing: 0,
                invalid: 0,
                stale: 0,
                orphaned: 0,
                allowlist_blocks: false,
                strict_blocks: false,
            },
            Case {
                name: "missing",
                profiles: |today, _| {
                    profile("ci", today, today).replace("owner = \"release/ci\"\n", "")
                },
                missing: 1,
                invalid: 0,
                stale: 0,
                orphaned: 0,
                allowlist_blocks: true,
                strict_blocks: true,
            },
            Case {
                name: "malformed",
                profiles: |_, _| profile("ci", "2026-02-30", "2026-08-12"),
                missing: 0,
                invalid: 1,
                stale: 0,
                orphaned: 0,
                allowlist_blocks: true,
                strict_blocks: true,
            },
            Case {
                name: "duplicate",
                profiles: |today, _| {
                    format!(
                        "{}{}",
                        profile("ci", today, today),
                        profile("ci", today, today)
                    )
                },
                missing: 0,
                invalid: 1,
                stale: 0,
                orphaned: 0,
                allowlist_blocks: true,
                strict_blocks: true,
            },
            Case {
                name: "stale-orphan",
                profiles: |today, yesterday| {
                    format!(
                        "{}{}",
                        profile("ci", yesterday, yesterday),
                        profile("orphan", today, today)
                    )
                },
                missing: 0,
                invalid: 0,
                stale: 1,
                orphaned: 1,
                allowlist_blocks: false,
                strict_blocks: true,
            },
        ];

        for kind in [ProfileKind::Process, ProfileKind::Network] {
            for case in cases {
                let root = unique_temp_dir(&format!("shipper-profile-{}-{:?}", case.name, kind));
                let _ = fs::remove_dir_all(&root);
                fs::create_dir_all(root.join(".github/workflows")).expect("workflow directory");
                fs::create_dir_all(root.join("policy")).expect("policy directory");
                fs::write(
                    root.join(".github/workflows/live.yml"),
                    "jobs:\n  test:\n    steps:\n      - run: cargo test && curl https://crates.io\n",
                )
                .expect("write workflow");
                fs::write(
                    root.join(WORKFLOW_ALLOWLIST),
                    r#"[[workflow]]
path = ".github/workflows/live.yml"
kind = "workflow"
process_policy = "ci"
network_policy = "ci"
"#,
                )
                .expect("write workflow ledger");
                let profile_doc = (case.profiles)(&today, &yesterday);
                fs::write(root.join(PROCESS_ALLOWLIST), &profile_doc)
                    .expect("write process ledger");
                fs::write(root.join(NETWORK_ALLOWLIST), &profile_doc)
                    .expect("write network ledger");
                let init = Command::new("git")
                    .arg("init")
                    .arg("--quiet")
                    .arg(&root)
                    .status()
                    .expect("initialize fixture repository");
                assert!(init.success());
                let add = Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["add", ".github/workflows/live.yml"])
                    .status()
                    .expect("track fixture workflow");
                assert!(add.success());

                let run = |mode| match kind {
                    ProfileKind::Process => check_process_policy_at(&root, mode),
                    ProfileKind::Network => check_network_policy_at(&root, mode),
                };
                assert!(run(Mode::Advisory).is_ok(), "{} {kind:?}", case.name);
                assert_eq!(
                    run(Mode::BlockingAllowlist).is_err(),
                    case.allowlist_blocks,
                    "{} {kind:?} allowlist",
                    case.name
                );
                assert_eq!(
                    run(Mode::BlockingStrict).is_err(),
                    case.strict_blocks,
                    "{} {kind:?} strict",
                    case.name
                );

                let basename = match kind {
                    ProfileKind::Process => "process-policy-report.json",
                    ProfileKind::Network => "network-policy-report.json",
                };
                let report: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(root.join(OUTPUT_DIR_REL).join(basename))
                        .expect("read adapter report"),
                )
                .expect("parse adapter report");
                assert_eq!(report["summary"]["missing_fields"], case.missing);
                assert_eq!(report["summary"]["invalid_profiles"], case.invalid);
                assert_eq!(report["summary"]["stale"], case.stale);
                assert_eq!(report["summary"]["orphaned"], case.orphaned);

                let _ = fs::remove_dir_all(&root);
            }
        }
    }

    /// A temp directory unique to this process and this call.
    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
