//! `cargo xtask check-doc-contracts --mode <mode>`
//!
//! Validates Shipper's source-of-truth document stack. The first pass is
//! advisory: write deterministic reports and exit zero, matching the policy
//! ladder used by the file-policy checks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const OUTPUT_DIR_REL: &str = "target/policy";
const MD_NAME: &str = "doc-contracts-report.md";
const JSON_NAME: &str = "doc-contracts-report.json";
const ACTIVE_GOAL_REL: &str = ".shipper-meta/goals/active.toml";

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
    work_item: Vec<WorkItem>,
}

#[derive(Debug, Deserialize)]
struct WorkItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    proposal: String,
    #[serde(default)]
    spec: String,
    #[serde(default)]
    plan: String,
}

pub fn check(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;
    let documents = collect_documents(&workspace_root)?;
    let mut findings = Vec::new();

    for document in &documents {
        findings.extend(check_document(&workspace_root, document)?);
    }

    let active_goal_checked = check_active_goal(&workspace_root, &mut findings)?;
    let blocking_findings = findings.iter().filter(|finding| finding.blocking).count();
    let report = Report {
        tool: "cargo xtask check-doc-contracts",
        mode: mode_str(mode),
        summary: Summary {
            documents_checked: documents.len(),
            active_goal_checked,
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

    for item in active_goal.work_item {
        for (field, value) in [
            ("proposal", item.proposal),
            ("spec", item.spec),
            ("plan", item.plan),
        ] {
            if value.trim().is_empty() {
                continue;
            }
            if !workspace_root.join(&value).exists() {
                let id = if item.id.is_empty() {
                    "<missing id>"
                } else {
                    item.id.as_str()
                };
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
        "doc-contracts ({}): documents={} active_goal={} findings={} blocking={}",
        report.mode,
        report.summary.documents_checked,
        report.summary.active_goal_checked,
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
}
