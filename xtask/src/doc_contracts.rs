//! `cargo xtask check-doc-contracts --mode <mode>`
//!
//! Validates Shipper's source-of-truth document stack. The first pass is
//! advisory: write deterministic reports and exit zero, matching the policy
//! ladder used by the file-policy checks.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const OUTPUT_DIR_REL: &str = "target/policy";
const MD_NAME: &str = "doc-contracts-report.md";
const JSON_NAME: &str = "doc-contracts-report.json";
const ACTIVE_GOAL_REL: &str = ".shipper-meta/goals/active.toml";
const WORKFLOW_INVENTORY_REL: &str = "docs/ci/test-evidence-lanes.md";
const SUPPORT_TIERS_REL: &str = "docs/status/SUPPORT_TIERS.md";

const REQUIRED_HEADERS: &[&str] = &[
    "Status",
    "Owner",
    "Created",
    "Milestone",
    "Linked proposal",
    "Linked specs",
    "Linked ADRs",
    "Linked plan",
    "Linked issues",
    "Linked PRs",
    "Support-tier impact",
    "Policy impact",
    "Proof commands",
];

const LINKED_FILE_HEADERS: &[&str] = &[
    "Linked proposal",
    "Linked specs",
    "Linked ADRs",
    "Linked plan",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    Advisory,
    BlockingAllowlist,
    BlockingStrict,
}

#[derive(Debug, Clone, Serialize)]
struct Report {
    tool: &'static str,
    mode: &'static str,
    summary: Summary,
    findings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    documents_checked: usize,
    active_goal_checked: bool,
    workflow_inventory_checked: bool,
    support_tiers_checked: bool,
    findings: usize,
    blocking_findings: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Finding {
    path: String,
    code: &'static str,
    message: String,
    blocking: bool,
}

#[derive(Debug, Clone)]
struct Document {
    path: PathBuf,
    rel: String,
    kind: DocumentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentKind {
    Proposal,
    Spec,
    Adr,
    Plan,
}

#[derive(Debug, Deserialize)]
struct ActiveGoal {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    created: String,
    #[serde(default)]
    objective: String,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default)]
    next_action: String,
    #[serde(default)]
    end_state: Vec<String>,
    #[serde(default)]
    work_item: Vec<WorkItem>,
}

#[derive(Debug, Deserialize)]
struct WorkItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    proposal: String,
    #[serde(default)]
    spec: String,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default)]
    next_action: String,
    #[serde(default)]
    commands: Vec<String>,
}

pub fn check(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;
    let documents = collect_documents(&workspace_root)?;
    let mut findings = Vec::new();

    for document in &documents {
        findings.extend(check_document(&workspace_root, document)?);
    }

    let active_goal_checked = check_active_goal(&workspace_root, &mut findings)?;
    let workflow_inventory_checked = check_workflow_inventory(&workspace_root, &mut findings)?;
    let support_tiers_checked = check_support_tiers(&workspace_root, &mut findings)?;
    let blocking_findings = findings.iter().filter(|finding| finding.blocking).count();
    let report = Report {
        tool: "cargo xtask check-doc-contracts",
        mode: mode_str(mode),
        summary: Summary {
            documents_checked: documents.len(),
            active_goal_checked,
            workflow_inventory_checked,
            support_tiers_checked,
            findings: findings.len(),
            blocking_findings,
        },
        findings,
    };

    write_report(&workspace_root, &report)?;
    print_stdout_summary(&report);

    if mode_fails(mode, &report) {
        bail!(
            "check-doc-contracts: {} mode found {} blocking issue(s); see {}/{}",
            report.mode,
            report.summary.blocking_findings,
            OUTPUT_DIR_REL,
            MD_NAME,
        );
    }

    Ok(())
}

fn collect_documents(workspace_root: &Path) -> Result<Vec<Document>> {
    let mut documents = Vec::new();
    collect_prefixed(
        workspace_root,
        "docs/proposals",
        "SHIPPER-PROP-",
        DocumentKind::Proposal,
        &mut documents,
    )?;
    collect_prefixed(
        workspace_root,
        "docs/specs",
        "SHIPPER-SPEC-",
        DocumentKind::Spec,
        &mut documents,
    )?;
    collect_prefixed(
        workspace_root,
        "docs/adr",
        "SHIPPER-ADR-",
        DocumentKind::Adr,
        &mut documents,
    )?;
    collect_plans(workspace_root, &mut documents)?;
    documents.sort_by(|left, right| left.rel.cmp(&right.rel));
    Ok(documents)
}

fn collect_prefixed(
    workspace_root: &Path,
    dir_rel: &str,
    prefix: &str,
    kind: DocumentKind,
    documents: &mut Vec<Document>,
) -> Result<()> {
    let dir = workspace_root.join(dir_rel);
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        documents.push(Document {
            rel: rel_path(workspace_root, &path)?,
            path,
            kind,
        });
    }
    Ok(())
}

fn collect_plans(workspace_root: &Path, documents: &mut Vec<Document>) -> Result<()> {
    let root = workspace_root.join("plans");
    if !root.exists() {
        return Ok(());
    }
    collect_plan_dir(workspace_root, &root, documents)
}

fn collect_plan_dir(
    workspace_root: &Path,
    dir: &Path,
    documents: &mut Vec<Document>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_plan_dir(workspace_root, &path, documents)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if matches!(name, "README.md" | "TEMPLATE.md") {
            continue;
        }
        documents.push(Document {
            rel: rel_path(workspace_root, &path)?,
            path,
            kind: DocumentKind::Plan,
        });
    }
    Ok(())
}

fn check_document(workspace_root: &Path, document: &Document) -> Result<Vec<Finding>> {
    let raw = fs::read_to_string(&document.path)
        .with_context(|| format!("reading {}", document.path.display()))?;
    let mut findings = Vec::new();
    let title = first_heading(&raw);
    let headers = parse_headers(&raw);

    if let Some(expected_id) = expected_id(document)
        && title_id(title) != Some(expected_id.as_str())
    {
        findings.push(Finding {
            path: document.rel.clone(),
            code: "id_mismatch",
            message: format!(
                "filename ID `{expected_id}` does not match title `{}`",
                title.unwrap_or("<missing title>")
            ),
            blocking: true,
        });
    }

    for key in REQUIRED_HEADERS {
        if !headers.contains_key(*key) {
            findings.push(Finding {
                path: document.rel.clone(),
                code: "missing_header",
                message: format!("missing required header `{key}`"),
                blocking: true,
            });
        }
    }

    match headers.get("Status").map(String::as_str) {
        Some(status) if valid_status(status) => {}
        Some(status) => findings.push(Finding {
            path: document.rel.clone(),
            code: "invalid_status",
            message: format!("invalid status `{status}`"),
            blocking: true,
        }),
        None => {}
    }

    for key in LINKED_FILE_HEADERS {
        if let Some(value) = headers.get(*key) {
            for linked in linked_paths(value) {
                if !workspace_root.join(&linked).exists() {
                    findings.push(Finding {
                        path: document.rel.clone(),
                        code: "missing_linked_file",
                        message: format!("`{key}` references missing file `{linked}`"),
                        blocking: true,
                    });
                }
            }
        }
    }

    Ok(findings)
}

fn first_heading(raw: &str) -> Option<&str> {
    raw.lines()
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
}

fn parse_headers(raw: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    let mut seen_title = false;
    for line in raw.lines() {
        if !seen_title {
            if line.starts_with("# ") {
                seen_title = true;
            }
            continue;
        }
        if line.starts_with("## ") {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if REQUIRED_HEADERS.contains(&key) {
            headers.insert(key.to_string(), value.trim().to_string());
        }
    }
    headers
}

fn expected_id(document: &Document) -> Option<String> {
    match document.kind {
        DocumentKind::Proposal | DocumentKind::Spec | DocumentKind::Adr => document
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split('-').take(3).collect::<Vec<_>>().join("-").into()),
        DocumentKind::Plan => None,
    }
}

fn title_id(title: Option<&str>) -> Option<&str> {
    title?.split(':').next().map(str::trim)
}

fn valid_status(status: &str) -> bool {
    matches!(
        status,
        "proposed" | "accepted" | "implemented" | "superseded"
    )
}

fn linked_paths(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .filter(|part| !part.starts_with('#'))
        .filter(|part| !part.eq_ignore_ascii_case("none"))
        .filter(|part| !part.to_ascii_lowercase().starts_with("future "))
        .filter(|part| part.contains('/'))
        .map(|part| part.trim_matches('`').to_string())
        .collect()
}

fn check_active_goal(workspace_root: &Path, findings: &mut Vec<Finding>) -> Result<bool> {
    let path = workspace_root.join(ACTIVE_GOAL_REL);
    if !path.exists() {
        findings.push(Finding {
            path: ACTIVE_GOAL_REL.to_string(),
            code: "missing_active_goal",
            message: "active goal manifest is missing".to_string(),
            blocking: true,
        });
        return Ok(false);
    }

    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let active_goal: ActiveGoal = match toml::from_str(&raw) {
        Ok(goal) => goal,
        Err(err) => {
            findings.push(Finding {
                path: ACTIVE_GOAL_REL.to_string(),
                code: "active_goal_parse_error",
                message: err.to_string(),
                blocking: true,
            });
            return Ok(true);
        }
    };

    check_active_goal_contract(&active_goal, findings);

    for item in active_goal.work_item {
        check_active_goal_work_item_contract(&item, findings);
        for (field, value) in [
            ("proposal", item.proposal.as_str()),
            ("spec", item.spec.as_str()),
            ("plan", item.plan.as_str()),
        ] {
            if value.trim().is_empty() {
                continue;
            }
            if !workspace_root.join(value).exists() {
                let id = active_goal_work_item_id(&item);
                findings.push(Finding {
                    path: ACTIVE_GOAL_REL.to_string(),
                    code: "active_goal_missing_link",
                    message: format!("work_item `{id}` {field} references missing file `{value}`"),
                    blocking: true,
                });
            }
        }
    }

    Ok(true)
}

fn check_workflow_inventory(workspace_root: &Path, findings: &mut Vec<Finding>) -> Result<bool> {
    let path = workspace_root.join(WORKFLOW_INVENTORY_REL);
    if !path.exists() {
        findings.push(Finding {
            path: WORKFLOW_INVENTORY_REL.to_string(),
            code: "missing_workflow_inventory",
            message: "workflow inventory document is missing".to_string(),
            blocking: true,
        });
        return Ok(false);
    }

    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let workflows = tracked_workflow_files(workspace_root)?;
    findings.extend(workflow_inventory_findings(&workflows, &raw));
    Ok(true)
}

fn check_support_tiers(workspace_root: &Path, findings: &mut Vec<Finding>) -> Result<bool> {
    let path = workspace_root.join(SUPPORT_TIERS_REL);
    if !path.exists() {
        findings.push(Finding {
            path: SUPPORT_TIERS_REL.to_string(),
            code: "missing_support_tiers",
            message: "support-tier claim map is missing".to_string(),
            blocking: true,
        });
        return Ok(false);
    }

    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    findings.extend(support_tier_metadata_findings(&raw, |linked| {
        workspace_root.join(linked).exists()
    }));
    Ok(true)
}

fn support_tier_metadata_findings<F>(raw: &str, mut linked_exists: F) -> Vec<Finding>
where
    F: FnMut(&str) -> bool,
{
    let headers = parse_headers(raw);
    let mut findings = Vec::new();

    for key in REQUIRED_HEADERS {
        if !headers.contains_key(*key) {
            findings.push(Finding {
                path: SUPPORT_TIERS_REL.to_string(),
                code: "support_tiers_missing_header",
                message: format!("missing required header `{key}`"),
                blocking: true,
            });
        }
    }

    match headers.get("Status").map(String::as_str) {
        Some(status) if valid_status(status) => {}
        Some(status) => findings.push(Finding {
            path: SUPPORT_TIERS_REL.to_string(),
            code: "support_tiers_invalid_status",
            message: format!("invalid status `{status}`"),
            blocking: true,
        }),
        None => {}
    }

    for key in LINKED_FILE_HEADERS {
        if let Some(value) = headers.get(*key) {
            for linked in linked_paths(value) {
                if !linked_exists(&linked) {
                    findings.push(Finding {
                        path: SUPPORT_TIERS_REL.to_string(),
                        code: "support_tiers_missing_linked_file",
                        message: format!("`{key}` references missing file `{linked}`"),
                        blocking: true,
                    });
                }
            }
        }
    }

    findings.extend(support_tier_claim_tier_findings(raw));

    findings
}

fn support_tier_claim_tier_findings(raw: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let allowed = support_tier_model_values(raw);
    if allowed.is_empty() {
        findings.push(Finding {
            path: SUPPORT_TIERS_REL.to_string(),
            code: "support_tiers_missing_tier_model",
            message: "support-tier claim map has no Tier Model values".to_string(),
            blocking: true,
        });
        return findings;
    }

    let claim_rows = markdown_table_rows(raw, "Claim Map");
    if claim_rows.is_empty() {
        findings.push(Finding {
            path: SUPPORT_TIERS_REL.to_string(),
            code: "support_tiers_missing_claim_map",
            message: "support-tier claim map has no Claim Map rows".to_string(),
            blocking: true,
        });
        return findings;
    }

    for row in claim_rows {
        if row.len() < 2 {
            continue;
        }
        let claim = &row[0];
        let tier = &row[1];
        if !allowed.contains(tier) {
            let allowed_values = allowed.iter().cloned().collect::<Vec<_>>().join(", ");
            findings.push(Finding {
                path: SUPPORT_TIERS_REL.to_string(),
                code: "support_tiers_invalid_claim_tier",
                message: format!(
                    "claim `{claim}` uses tier `{tier}`, but Tier Model defines: {allowed_values}"
                ),
                blocking: true,
            });
        }
    }

    findings
}

fn support_tier_model_values(raw: &str) -> BTreeSet<String> {
    markdown_table_rows(raw, "Tier Model")
        .into_iter()
        .filter_map(|row| row.first().cloned())
        .collect()
}

fn markdown_table_rows(raw: &str, section: &str) -> Vec<Vec<String>> {
    let section_heading = format!("## {section}");
    let mut in_section = false;
    let mut seen_header = false;
    let mut rows = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == section_heading {
            in_section = true;
            continue;
        }
        if in_section && trimmed.starts_with("## ") {
            break;
        }
        if !in_section {
            continue;
        }
        let Some(row) = markdown_table_row(trimmed) else {
            continue;
        };
        if !seen_header {
            seen_header = true;
            continue;
        }
        rows.push(row);
    }

    rows
}

fn markdown_table_row(line: &str) -> Option<Vec<String>> {
    if !line.starts_with('|') || !line.ends_with('|') {
        return None;
    }
    let cells = line
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect::<Vec<_>>();
    if cells.is_empty()
        || cells
            .iter()
            .all(|cell| cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
    {
        return None;
    }
    Some(cells)
}

fn workflow_inventory_findings(workflow_paths: &[String], inventory_raw: &str) -> Vec<Finding> {
    let tracked: BTreeSet<String> = workflow_paths
        .iter()
        .filter_map(|path| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .collect();
    let listed = workflow_names_in_inventory(inventory_raw);
    let mut findings = Vec::new();

    for workflow in tracked.difference(&listed) {
        findings.push(Finding {
            path: WORKFLOW_INVENTORY_REL.to_string(),
            code: "workflow_inventory_missing",
            message: format!(
                "tracked workflow `{workflow}` is missing from the workflow inventory"
            ),
            blocking: true,
        });
    }

    for workflow in listed.difference(&tracked) {
        findings.push(Finding {
            path: WORKFLOW_INVENTORY_REL.to_string(),
            code: "workflow_inventory_stale",
            message: format!(
                "workflow inventory lists `{workflow}`, but no tracked workflow file exists"
            ),
            blocking: true,
        });
    }

    findings
}

fn workflow_names_in_inventory(raw: &str) -> BTreeSet<String> {
    let mut in_inventory = false;
    let mut names = BTreeSet::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == "## Workflow Inventory" {
            in_inventory = true;
            continue;
        }
        if in_inventory && trimmed.starts_with("## ") {
            break;
        }
        if !in_inventory || !trimmed.starts_with('|') {
            continue;
        }

        for name in backticked_segments(trimmed).filter(|segment| segment.ends_with(".yml")) {
            names.insert(name.to_string());
        }
    }

    names
}

fn backticked_segments(line: &str) -> impl Iterator<Item = &str> {
    line.split('`')
        .enumerate()
        .filter_map(|(index, segment)| (index % 2 == 1).then_some(segment))
}

fn check_active_goal_contract(goal: &ActiveGoal, findings: &mut Vec<Finding>) {
    for (field, value) in [
        ("id", goal.id.as_str()),
        ("title", goal.title.as_str()),
        ("status", goal.status.as_str()),
        ("owner", goal.owner.as_str()),
        ("created", goal.created.as_str()),
        ("objective", goal.objective.as_str()),
    ] {
        if value.trim().is_empty() {
            findings.push(Finding {
                path: ACTIVE_GOAL_REL.to_string(),
                code: "active_goal_missing_field",
                message: format!("missing required field `{field}`"),
                blocking: true,
            });
        }
    }

    if !goal.status.trim().is_empty() && !valid_active_goal_status(&goal.status) {
        findings.push(Finding {
            path: ACTIVE_GOAL_REL.to_string(),
            code: "active_goal_invalid_status",
            message: format!("invalid status `{}`", goal.status),
            blocking: true,
        });
    }

    if !has_non_empty_value(&goal.end_state) {
        findings.push(Finding {
            path: ACTIVE_GOAL_REL.to_string(),
            code: "active_goal_missing_field",
            message: "missing required field `end_state`".to_string(),
            blocking: true,
        });
    }

    if goal.status == "blocked" {
        if !has_non_empty_value(&goal.blocked_by) {
            findings.push(Finding {
                path: ACTIVE_GOAL_REL.to_string(),
                code: "active_goal_blocked_without_blocker",
                message: "active goal is blocked but does not define blocked_by".to_string(),
                blocking: true,
            });
        }
        if goal.next_action.trim().is_empty() {
            findings.push(Finding {
                path: ACTIVE_GOAL_REL.to_string(),
                code: "active_goal_blocked_without_next_action",
                message: "active goal is blocked but does not define next_action".to_string(),
                blocking: true,
            });
        }
    }
}

fn valid_active_goal_status(status: &str) -> bool {
    matches!(status, "active" | "blocked" | "complete")
}

fn check_active_goal_work_item_contract(item: &WorkItem, findings: &mut Vec<Finding>) {
    let id = active_goal_work_item_id(item);
    for (field, value) in [("id", item.id.as_str()), ("status", item.status.as_str())] {
        if value.trim().is_empty() {
            findings.push(Finding {
                path: ACTIVE_GOAL_REL.to_string(),
                code: "active_goal_work_item_missing_field",
                message: format!("work_item `{id}` missing required field `{field}`"),
                blocking: true,
            });
        }
    }

    if !item.status.trim().is_empty() && !valid_active_goal_work_item_status(&item.status) {
        findings.push(Finding {
            path: ACTIVE_GOAL_REL.to_string(),
            code: "active_goal_work_item_invalid_status",
            message: format!("work_item `{id}` has invalid status `{}`", item.status),
            blocking: true,
        });
    }

    match item.status.as_str() {
        "blocked" => {
            if !has_non_empty_value(&item.blocked_by) {
                findings.push(Finding {
                    path: ACTIVE_GOAL_REL.to_string(),
                    code: "active_goal_blocked_without_blocker",
                    message: format!("work_item `{id}` is blocked but does not define blocked_by"),
                    blocking: true,
                });
            }
            if item.next_action.trim().is_empty() {
                findings.push(Finding {
                    path: ACTIVE_GOAL_REL.to_string(),
                    code: "active_goal_blocked_without_next_action",
                    message: format!("work_item `{id}` is blocked but does not define next_action"),
                    blocking: true,
                });
            }
        }
        "ready" | "active" if !has_non_empty_value(&item.commands) => {
            findings.push(Finding {
                path: ACTIVE_GOAL_REL.to_string(),
                code: "active_goal_work_item_without_proof_commands",
                message: format!(
                    "work_item `{id}` is {} but does not define proof commands",
                    item.status
                ),
                blocking: true,
            });
        }
        "planned" if !has_non_empty_value(&item.commands) => {
            findings.push(Finding {
                path: ACTIVE_GOAL_REL.to_string(),
                code: "active_goal_planned_without_proof_commands",
                message: format!("work_item `{id}` is planned but does not define proof commands"),
                blocking: true,
            });
        }
        _ => {}
    }
}

fn valid_active_goal_work_item_status(status: &str) -> bool {
    matches!(
        status,
        "ready" | "active" | "planned" | "blocked" | "complete"
    )
}

fn active_goal_work_item_id(item: &WorkItem) -> &str {
    if item.id.trim().is_empty() {
        "<missing id>"
    } else {
        item.id.as_str()
    }
}

fn has_non_empty_value(values: &[String]) -> bool {
    values.iter().any(|value| !value.trim().is_empty())
}

fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Advisory => "advisory",
        Mode::BlockingAllowlist => "blocking-allowlist",
        Mode::BlockingStrict => "blocking-strict",
    }
}

fn mode_fails(mode: Mode, report: &Report) -> bool {
    match mode {
        Mode::Advisory => false,
        Mode::BlockingAllowlist | Mode::BlockingStrict => report.summary.blocking_findings > 0,
    }
}

fn write_report(workspace_root: &Path, report: &Report) -> Result<()> {
    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json = serde_json::to_string_pretty(report).context("serializing report as JSON")?;
    fs::write(out_dir.join(JSON_NAME), json).context("writing doc-contracts JSON report")?;
    fs::write(out_dir.join(MD_NAME), render_markdown(report))
        .context("writing doc-contracts Markdown report")?;
    Ok(())
}

fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Doc-Contracts Report\n\n");
    out.push_str(&format!(
        "Generated by `{} --mode {}`.\n\n",
        report.tool, report.mode
    ));
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Documents checked: {}\n",
        report.summary.documents_checked
    ));
    out.push_str(&format!(
        "- Active goal checked: {}\n",
        report.summary.active_goal_checked
    ));
    out.push_str(&format!(
        "- Workflow inventory checked: {}\n",
        report.summary.workflow_inventory_checked
    ));
    out.push_str(&format!(
        "- Support tiers checked: {}\n",
        report.summary.support_tiers_checked
    ));
    out.push_str(&format!("- Findings: {}\n", report.summary.findings));
    out.push_str(&format!(
        "- Blocking findings: {}\n\n",
        report.summary.blocking_findings
    ));

    out.push_str("## Findings\n\n");
    if report.findings.is_empty() {
        out.push_str("_(none)_\n");
    } else {
        for finding in &report.findings {
            out.push_str(&format!(
                "- `{}` `{}`: {}\n",
                finding.path, finding.code, finding.message
            ));
        }
    }
    out
}

fn print_stdout_summary(report: &Report) {
    println!(
        "doc-contracts ({}): documents={} active_goal={} workflow_inventory={} support_tiers={} findings={} blocking={}",
        report.mode,
        report.summary.documents_checked,
        report.summary.active_goal_checked,
        report.summary.workflow_inventory_checked,
        report.summary.support_tiers_checked,
        report.summary.findings,
        report.summary.blocking_findings,
    );
}

fn rel_path(workspace_root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(workspace_root)
        .with_context(|| format!("{} is outside {}", path.display(), workspace_root.display()))?
        .to_string_lossy()
        .replace('\\', "/"))
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
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .filter(|path| path.starts_with(".github/workflows/") && path.ends_with(".yml"))
        .collect();
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_id_reads_prefix_before_colon() {
        assert_eq!(
            title_id(Some("SHIPPER-SPEC-0001: Source-of-Truth Stack")),
            Some("SHIPPER-SPEC-0001")
        );
    }

    #[test]
    fn linked_paths_ignores_issues_and_future_text() {
        let paths = linked_paths(
            "docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md, #109, future docs/specs/SHIPPER-SPEC-0002.md",
        );
        assert_eq!(
            paths,
            vec!["docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md"]
        );
    }

    #[test]
    fn valid_status_accepts_contract_values_only() {
        assert!(valid_status("proposed"));
        assert!(valid_status("accepted"));
        assert!(valid_status("implemented"));
        assert!(valid_status("superseded"));
        assert!(!valid_status("active"));
    }

    #[test]
    fn parse_headers_stops_at_first_section() {
        let raw = "\
# Title

Status: accepted
Owner: EffortlessMetrics

## Body

Status: proposed
";
        let headers = parse_headers(raw);
        assert_eq!(headers.get("Status").map(String::as_str), Some("accepted"));
    }

    #[test]
    fn workflow_inventory_names_reads_backticked_yml_names() {
        let raw = "\
Outside the inventory, `.github/workflows/*.yml` is just prose.

## Workflow Inventory

| Workflow | Trigger |
|---|---|
| `ci.yml` | `push` |
| `release.yml` and `ripr.yml` | `workflow_dispatch` |
| `not-a-workflow.txt` | example |

## Required PR Gate

Outside the inventory, `.github/workflows/ripr.yml` is just prose.
";

        assert_eq!(
            workflow_names_in_inventory(raw),
            BTreeSet::from([
                "ci.yml".to_string(),
                "release.yml".to_string(),
                "ripr.yml".to_string()
            ])
        );
    }

    #[test]
    fn workflow_inventory_findings_report_missing_and_stale_entries() {
        let workflow_paths = vec![
            ".github/workflows/ci.yml".to_string(),
            ".github/workflows/release.yml".to_string(),
        ];
        let raw = "\
## Workflow Inventory

| Workflow | Trigger |
|---|---|
| `ci.yml` | `push` |
| `old.yml` | `workflow_dispatch` |
";

        let findings = workflow_inventory_findings(&workflow_paths, raw);
        let codes_and_messages = findings
            .iter()
            .map(|finding| (finding.code, finding.message.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            codes_and_messages,
            vec![
                (
                    "workflow_inventory_missing",
                    "tracked workflow `release.yml` is missing from the workflow inventory"
                ),
                (
                    "workflow_inventory_stale",
                    "workflow inventory lists `old.yml`, but no tracked workflow file exists"
                )
            ]
        );
    }

    #[test]
    fn support_tier_metadata_findings_report_missing_header_invalid_status_and_link() {
        let raw = "\
# Support Tiers

Status: active
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/missing.md
Linked ADRs:
Linked plan:
Linked issues: #109
Linked PRs:
Support-tier impact: source of truth
Policy impact: policy ledgers
Proof commands: cargo xtask policy-report

## Tier Model
";

        let findings =
            support_tier_metadata_findings(raw, |linked| linked != "docs/specs/missing.md");
        let codes_and_messages = findings
            .iter()
            .map(|finding| (finding.code, finding.message.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            codes_and_messages,
            vec![
                (
                    "support_tiers_missing_header",
                    "missing required header `Owner`"
                ),
                ("support_tiers_invalid_status", "invalid status `active`"),
                (
                    "support_tiers_missing_linked_file",
                    "`Linked specs` references missing file `docs/specs/missing.md`"
                ),
                (
                    "support_tiers_missing_tier_model",
                    "support-tier claim map has no Tier Model values"
                )
            ]
        );
    }

    #[test]
    fn support_tier_claim_tier_findings_require_tiers_from_model() {
        let raw = "\
# Support Tiers

## Tier Model

| Tier | Meaning |
|---|---|
| stable | Implemented and tested. |
| advisory | Useful non-blocking signal. |

## Claim Map

| Claim | Tier | Proof / Source | Owner |
|---|---|---|---|
| Install facade | stable | proof | packaging |
| Token fallback default | planned/advisory | proof | release |
";

        let findings = support_tier_claim_tier_findings(raw);
        let codes_and_messages = findings
            .iter()
            .map(|finding| (finding.code, finding.message.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            codes_and_messages,
            vec![(
                "support_tiers_invalid_claim_tier",
                "claim `Token fallback default` uses tier `planned/advisory`, but Tier Model defines: advisory, stable"
            )]
        );
    }

    #[test]
    fn active_goal_contract_requires_required_metadata() {
        let goal = ActiveGoal {
            id: String::new(),
            title: String::new(),
            status: "done".to_string(),
            owner: String::new(),
            created: String::new(),
            objective: String::new(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            end_state: Vec::new(),
            work_item: Vec::new(),
        };

        let mut findings = Vec::new();
        check_active_goal_contract(&goal, &mut findings);

        let codes_and_messages = findings
            .iter()
            .map(|finding| (finding.code, finding.message.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            codes_and_messages,
            vec![
                ("active_goal_missing_field", "missing required field `id`"),
                (
                    "active_goal_missing_field",
                    "missing required field `title`"
                ),
                (
                    "active_goal_missing_field",
                    "missing required field `owner`"
                ),
                (
                    "active_goal_missing_field",
                    "missing required field `created`"
                ),
                (
                    "active_goal_missing_field",
                    "missing required field `objective`"
                ),
                ("active_goal_invalid_status", "invalid status `done`"),
                (
                    "active_goal_missing_field",
                    "missing required field `end_state`"
                )
            ]
        );
    }

    #[test]
    fn active_goal_contract_accepts_current_status_values() {
        let mut goal = ActiveGoal {
            id: "shipper-swarm-development-control-plane".to_string(),
            title: "shipper-swarm development control plane".to_string(),
            status: "active".to_string(),
            owner: "EffortlessMetrics".to_string(),
            created: "2026-05-24".to_string(),
            objective: "Keep the swarm queue moving.".to_string(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            end_state: vec!["Queue is clean.".to_string()],
            work_item: Vec::new(),
        };

        let mut findings = Vec::new();
        check_active_goal_contract(&goal, &mut findings);
        assert!(findings.is_empty());

        goal.status = "blocked".to_string();
        goal.blocked_by = vec!["external proof missing".to_string()];
        goal.next_action = "collect proof".to_string();
        check_active_goal_contract(&goal, &mut findings);
        assert!(findings.is_empty());

        goal.status = "complete".to_string();
        check_active_goal_contract(&goal, &mut findings);
        assert!(findings.is_empty());
    }

    #[test]
    fn blocked_active_goals_require_blocker_and_next_action() {
        let goal = ActiveGoal {
            id: "shipper-trusted-publishing-default-proof".to_string(),
            title: "Trusted Publishing default proof".to_string(),
            status: "blocked".to_string(),
            owner: "EffortlessMetrics".to_string(),
            created: "2026-05-24".to_string(),
            objective: "Prove the release auth default.".to_string(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            end_state: vec!["Trusted Publishing evidence is recorded.".to_string()],
            work_item: Vec::new(),
        };

        let mut findings = Vec::new();
        check_active_goal_contract(&goal, &mut findings);

        let codes_and_messages = findings
            .iter()
            .map(|finding| (finding.code, finding.message.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            codes_and_messages,
            vec![
                (
                    "active_goal_blocked_without_blocker",
                    "active goal is blocked but does not define blocked_by"
                ),
                (
                    "active_goal_blocked_without_next_action",
                    "active goal is blocked but does not define next_action"
                )
            ]
        );
    }

    #[test]
    fn blocked_active_goal_items_require_blocker_and_next_action() {
        let item = WorkItem {
            id: "release-auth".to_string(),
            status: "blocked".to_string(),
            proposal: String::new(),
            spec: String::new(),
            plan: String::new(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            commands: Vec::new(),
        };

        let mut findings = Vec::new();
        check_active_goal_work_item_contract(&item, &mut findings);

        let codes = findings
            .iter()
            .map(|finding| finding.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec![
                "active_goal_blocked_without_blocker",
                "active_goal_blocked_without_next_action"
            ]
        );
    }

    #[test]
    fn active_goal_work_items_require_id_and_status() {
        let item = WorkItem {
            id: String::new(),
            status: String::new(),
            proposal: String::new(),
            spec: String::new(),
            plan: String::new(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            commands: Vec::new(),
        };

        let mut findings = Vec::new();
        check_active_goal_work_item_contract(&item, &mut findings);

        let codes_and_messages = findings
            .iter()
            .map(|finding| (finding.code, finding.message.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            codes_and_messages,
            vec![
                (
                    "active_goal_work_item_missing_field",
                    "work_item `<missing id>` missing required field `id`"
                ),
                (
                    "active_goal_work_item_missing_field",
                    "work_item `<missing id>` missing required field `status`"
                )
            ]
        );
    }

    #[test]
    fn active_goal_work_items_reject_unknown_statuses() {
        let item = WorkItem {
            id: "release-auth".to_string(),
            status: "done".to_string(),
            proposal: String::new(),
            spec: String::new(),
            plan: String::new(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            commands: Vec::new(),
        };

        let mut findings = Vec::new();
        check_active_goal_work_item_contract(&item, &mut findings);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "active_goal_work_item_invalid_status");
        assert_eq!(
            findings[0].message,
            "work_item `release-auth` has invalid status `done`"
        );
    }

    #[test]
    fn ready_and_active_work_items_require_proof_commands() {
        for status in ["ready", "active"] {
            let item = WorkItem {
                id: format!("{status}-work"),
                status: status.to_string(),
                proposal: String::new(),
                spec: String::new(),
                plan: String::new(),
                blocked_by: Vec::new(),
                next_action: String::new(),
                commands: Vec::new(),
            };

            let mut findings = Vec::new();
            check_active_goal_work_item_contract(&item, &mut findings);

            assert_eq!(findings.len(), 1);
            assert_eq!(
                findings[0].code,
                "active_goal_work_item_without_proof_commands"
            );
            assert_eq!(
                findings[0].message,
                format!("work_item `{status}-work` is {status} but does not define proof commands")
            );
        }
    }

    #[test]
    fn planned_active_goal_items_require_proof_commands() {
        let item = WorkItem {
            id: "support-tier-promotion".to_string(),
            status: "planned".to_string(),
            proposal: String::new(),
            spec: String::new(),
            plan: String::new(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            commands: Vec::new(),
        };

        let mut findings = Vec::new();
        check_active_goal_work_item_contract(&item, &mut findings);

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].code,
            "active_goal_planned_without_proof_commands"
        );
    }

    #[test]
    fn planned_active_goal_items_accept_non_empty_proof_commands() {
        let item = WorkItem {
            id: "support-tier-promotion".to_string(),
            status: "planned".to_string(),
            proposal: String::new(),
            spec: String::new(),
            plan: String::new(),
            blocked_by: Vec::new(),
            next_action: String::new(),
            commands: vec!["cargo xtask policy-report".to_string()],
        };

        let mut findings = Vec::new();
        check_active_goal_work_item_contract(&item, &mut findings);

        assert!(findings.is_empty());
    }
}
