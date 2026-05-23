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

    let findings = WorkflowFindings {
        unreceipted,
        missing_fields,
        expired,
        stale,
        unused,
        invalid_policy_refs,
        repository_guard_violations,
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
        "{} ({}): workflows={} entries={} unreceipted={} missing_fields={} expired={} stale={} unused={} invalid_refs={} repository_guard_violations={}",
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
    let mut n = f.unreceipted.len()
        + f.missing_fields.len()
        + f.expired.len()
        + f.invalid_policy_refs.len()
        + f.repository_guard_violations.len();
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
    out
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

fn workflow_job_blocks(yaml_text: &str) -> Vec<(String, String)> {
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
    let single_quoted = format!("github.repository == '{required_repository}'");
    let double_quoted = format!("github.repository == \"{required_repository}\"");
    block.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("if:")
            && (trimmed.contains(&single_quoted) || trimmed.contains(&double_quoted))
    })
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
    workflows: Vec<PerWorkflowReport>,
}

#[derive(Debug, Clone, Serialize)]
struct ScanSummary {
    workflows: usize,
    unknown_total: usize,
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
    let entries = load_workflow_allowlist(&workspace_root)?;
    let profiles_by_name = load_profiles(&workspace_root, PROCESS_ALLOWLIST)?;
    let today = today_iso();

    let mut per_workflow = Vec::new();
    let mut unknown_total = 0usize;
    for e in &entries {
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

        let content = read_workflow_content(&workspace_root, path).unwrap_or_default();
        let detected = detect_commands_in_runs(&content, KNOWN_COMMANDS);
        let unknown: Vec<String> = detected
            .iter()
            .filter(|c| !allowed.contains(c.as_str()))
            .cloned()
            .collect();
        unknown_total += unknown.len();

        per_workflow.push(PerWorkflowReport {
            workflow: path.clone(),
            declared_profile: profile,
            detected,
            unknown,
        });
    }

    let report = ScanReport {
        tool: "cargo xtask check-process-policy",
        mode: mode_str(mode),
        today,
        summary: ScanSummary {
            workflows: per_workflow.len(),
            unknown_total,
        },
        workflows: per_workflow,
    };
    write_scan_report(&workspace_root, "process-policy-report", &report)?;
    println!(
        "{} ({}): workflows={} unknown_total={}",
        report.tool, report.mode, report.summary.workflows, report.summary.unknown_total
    );

    if !matches!(mode, Mode::Advisory) && unknown_total > 0 {
        bail!(
            "{}: {} mode found {} unknown command(s) across {} workflow(s)",
            report.tool,
            report.mode,
            unknown_total,
            report.summary.workflows
        );
    }
    Ok(())
}

// ─── check-network-policy ───────────────────────────────────────────────────

pub fn check_network_policy(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;
    let entries = load_workflow_allowlist(&workspace_root)?;
    let profiles_by_name = load_profiles(&workspace_root, NETWORK_ALLOWLIST)?;
    let today = today_iso();
    let host_re =
        Regex::new(r"https?://([A-Za-z0-9.\-]+)").context("compiling network hostname regex")?;

    let mut per_workflow = Vec::new();
    let mut unknown_total = 0usize;
    for e in &entries {
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

        let content = read_workflow_content(&workspace_root, path).unwrap_or_default();
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
        unknown_total += unknown.len();

        per_workflow.push(PerWorkflowReport {
            workflow: path.clone(),
            declared_profile: profile,
            detected: detected_vec,
            unknown,
        });
    }

    let report = ScanReport {
        tool: "cargo xtask check-network-policy",
        mode: mode_str(mode),
        today,
        summary: ScanSummary {
            workflows: per_workflow.len(),
            unknown_total,
        },
        workflows: per_workflow,
    };
    write_scan_report(&workspace_root, "network-policy-report", &report)?;
    println!(
        "{} ({}): workflows={} unknown_total={}",
        report.tool, report.mode, report.summary.workflows, report.summary.unknown_total
    );

    if !matches!(mode, Mode::Advisory) && unknown_total > 0 {
        bail!(
            "{}: {} mode found {} unknown endpoint(s) across {} workflow(s)",
            report.tool,
            report.mode,
            unknown_total,
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
    let profiles = load_profiles(workspace_root, rel)?;
    Ok(profiles.keys().cloned().collect())
}

fn load_profiles(workspace_root: &Path, rel: &str) -> Result<BTreeMap<String, RawProfile>> {
    let path = workspace_root.join(rel);
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc: ProfileDoc =
        toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))?;
    let mut by_name = BTreeMap::new();
    for p in doc.profile {
        if let Some(name) = p.name.clone() {
            by_name.insert(name, p);
        }
    }
    Ok(by_name)
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
    out.push_str(&format!(
        "- Unknown commands/endpoints total: {}\n\n",
        r.summary.unknown_total
    ));
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
}
