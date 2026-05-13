//! `cargo xtask check-doc-contracts --mode <mode>`
//!
//! Validates Shipper's source-of-truth document graph. The first rollout is
//! intentionally modest: it checks IDs, required headers, status values,
//! linked artifacts, and the active goal manifest.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const OUTPUT_DIR_REL: &str = "target/policy";
const MD_NAME: &str = "doc-contracts-report.md";
const JSON_NAME: &str = "doc-contracts-report.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    Advisory,
    BlockingAllowlist,
    BlockingStrict,
}

#[derive(Debug, Clone, Copy)]
enum DocumentKind {
    Proposal,
    Spec,
    Adr,
    Plan,
}

impl DocumentKind {
    fn label(self) -> &'static str {
        match self {
            Self::Proposal => "proposal",
            Self::Spec => "spec",
            Self::Adr => "adr",
            Self::Plan => "plan",
        }
    }

    fn id_prefix(self) -> Option<&'static str> {
        match self {
            Self::Proposal => Some("SHIPPER-PROP-"),
            Self::Spec => Some("SHIPPER-SPEC-"),
            Self::Adr => Some("SHIPPER-ADR-"),
            Self::Plan => None,
        }
    }

    fn required_headers(self) -> &'static [&'static str] {
        match self {
            Self::Proposal => &[
                "Status",
                "Owner",
                "Created",
                "Target milestone",
                "Linked specs",
                "Linked ADRs",
                "Linked plan",
                "Linked issues",
                "Linked PRs",
                "Support-tier impact",
                "Policy impact",
            ],
            Self::Spec => &[
                "Status",
                "Owner",
                "Created",
                "Linked proposal",
                "Linked ADRs",
                "Linked plan",
                "Linked issues",
                "Linked PRs",
                "Support-tier impact",
                "Policy impact",
            ],
            Self::Adr => &[
                "Status",
                "Date",
                "Owner",
                "Linked proposal",
                "Linked specs",
                "Linked plan",
            ],
            Self::Plan => &[
                "Status",
                "Owner",
                "Milestone",
                "Linked proposal",
                "Linked specs",
                "Linked ADRs",
                "Linked issues",
            ],
        }
    }
}

#[derive(Debug, Clone)]
struct Document {
    kind: DocumentKind,
    rel_path: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
struct MissingHeaders {
    path: String,
    kind: &'static str,
    missing: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
struct InvalidId {
    path: String,
    expected: String,
    found: String,
}

#[derive(Debug, Clone, Serialize)]
struct InvalidStatus {
    path: String,
    status: String,
}

#[derive(Debug, Clone, Serialize)]
struct BrokenLink {
    path: String,
    header: String,
    target: String,
}

#[derive(Debug, Clone, Serialize)]
struct ParseError {
    path: String,
    error: String,
}

#[derive(Debug, Clone, Serialize)]
struct Findings {
    missing_headers: Vec<MissingHeaders>,
    invalid_ids: Vec<InvalidId>,
    invalid_statuses: Vec<InvalidStatus>,
    broken_links: Vec<BrokenLink>,
    parse_errors: Vec<ParseError>,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    documents: usize,
    active_work_items: usize,
    missing_headers: usize,
    invalid_ids: usize,
    invalid_statuses: usize,
    broken_links: usize,
    parse_errors: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Report {
    tool: &'static str,
    mode: &'static str,
    generated_at: String,
    summary: Summary,
    findings: Findings,
}

#[derive(Debug, Deserialize)]
struct ActiveGoal {
    #[serde(default)]
    work_item: Vec<ActiveWorkItem>,
}

#[derive(Debug, Deserialize)]
struct ActiveWorkItem {
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
    let mut findings = Findings {
        missing_headers: Vec::new(),
        invalid_ids: Vec::new(),
        invalid_statuses: Vec::new(),
        broken_links: Vec::new(),
        parse_errors: Vec::new(),
    };

    for document in &documents {
        check_document(&workspace_root, document, &mut findings);
    }
    let active_work_items = check_active_goal(&workspace_root, &mut findings)?;

    let summary = Summary {
        documents: documents.len(),
        active_work_items,
        missing_headers: findings.missing_headers.len(),
        invalid_ids: findings.invalid_ids.len(),
        invalid_statuses: findings.invalid_statuses.len(),
        broken_links: findings.broken_links.len(),
        parse_errors: findings.parse_errors.len(),
    };

    let report = Report {
        tool: "cargo xtask check-doc-contracts",
        mode: mode_str(mode),
        generated_at: today_iso(),
        summary,
        findings,
    };

    write_report(&workspace_root, &report)?;
    print_stdout_summary(&report);

    if mode_fails(mode, &report.findings) {
        bail!(
            "check-doc-contracts: {} mode found {} blocking issue(s); see {}/{}",
            report.mode,
            blocking_count(&report.findings),
            OUTPUT_DIR_REL,
            MD_NAME,
        );
    }

    Ok(())
}

fn check_document(workspace_root: &Path, document: &Document, findings: &mut Findings) {
    let headers = parse_headers(&document.content);
    let missing: Vec<&'static str> = document
        .kind
        .required_headers()
        .iter()
        .copied()
        .filter(|key| !headers.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        findings.missing_headers.push(MissingHeaders {
            path: document.rel_path.clone(),
            kind: document.kind.label(),
            missing,
        });
    }

    if let Some(prefix) = document.kind.id_prefix() {
        check_id(document, prefix, findings);
    }

    if let Some(status) = headers.get("Status") {
        let normalized = status.trim().to_ascii_lowercase();
        if !is_valid_status(&normalized) {
            findings.invalid_statuses.push(InvalidStatus {
                path: document.rel_path.clone(),
                status: status.clone(),
            });
        }
    }

    for (header, value) in &headers {
        if header.starts_with("Linked ") {
            check_links(workspace_root, document, header, value, findings);
        }
    }
}

fn check_id(document: &Document, prefix: &str, findings: &mut Findings) {
    let expected = match filename_id(&document.rel_path, prefix) {
        Some(id) => id,
        None => {
            findings.invalid_ids.push(InvalidId {
                path: document.rel_path.clone(),
                expected: format!("{prefix}NNNN"),
                found: document.rel_path.clone(),
            });
            return;
        }
    };

    let title = document
        .content
        .lines()
        .find(|line| line.starts_with("# "))
        .unwrap_or_default()
        .trim();
    if !title.starts_with(&format!("# {expected}:")) {
        findings.invalid_ids.push(InvalidId {
            path: document.rel_path.clone(),
            expected,
            found: title.to_string(),
        });
    }
}

fn check_active_goal(workspace_root: &Path, findings: &mut Findings) -> Result<usize> {
    let rel_path = ".shipper-meta/goals/active.toml";
    let path = workspace_root.join(rel_path);
    if !path.exists() {
        return Ok(0);
    }

    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let goal = match toml::from_str::<ActiveGoal>(&raw) {
        Ok(goal) => goal,
        Err(error) => {
            findings.parse_errors.push(ParseError {
                path: rel_path.to_string(),
                error: error.to_string(),
            });
            return Ok(0);
        }
    };

    for item in &goal.work_item {
        check_active_reference(
            workspace_root,
            rel_path,
            &item.id,
            "proposal",
            &item.proposal,
            findings,
        );
        check_active_reference(
            workspace_root,
            rel_path,
            &item.id,
            "spec",
            &item.spec,
            findings,
        );
        check_active_reference(
            workspace_root,
            rel_path,
            &item.id,
            "plan",
            &item.plan,
            findings,
        );
    }

    Ok(goal.work_item.len())
}

fn check_active_reference(
    workspace_root: &Path,
    rel_path: &str,
    item_id: &str,
    field: &str,
    value: &str,
    findings: &mut Findings,
) {
    if value.trim().is_empty() {
        return;
    }
    if !target_exists(workspace_root, value.trim()) {
        findings.broken_links.push(BrokenLink {
            path: rel_path.to_string(),
            header: format!("work_item[{item_id}].{field}"),
            target: value.trim().to_string(),
        });
    }
}

fn check_links(
    workspace_root: &Path,
    document: &Document,
    header: &str,
    value: &str,
    findings: &mut Findings,
) {
    for target in split_link_targets(value) {
        if target.is_empty() || target.starts_with('#') || target.eq_ignore_ascii_case("none") {
            continue;
        }
        if !target_exists(workspace_root, &target) {
            findings.broken_links.push(BrokenLink {
                path: document.rel_path.clone(),
                header: header.to_string(),
                target,
            });
        }
    }
}

fn target_exists(workspace_root: &Path, target: &str) -> bool {
    let target = target
        .trim()
        .trim_matches('`')
        .trim_end_matches('.')
        .trim_end_matches(';');
    if target.is_empty() {
        return true;
    }

    if target.starts_with("docs/")
        || target.starts_with("plans/")
        || target.starts_with("policy/")
        || target.starts_with(".shipper-meta/")
    {
        return workspace_root.join(target).exists();
    }
    if let Some(prefix) = target.strip_prefix("SHIPPER-PROP-") {
        return artifact_with_id_exists(
            workspace_root,
            "docs/proposals",
            &format!("SHIPPER-PROP-{prefix}"),
        );
    }
    if let Some(prefix) = target.strip_prefix("SHIPPER-SPEC-") {
        return artifact_with_id_exists(
            workspace_root,
            "docs/specs",
            &format!("SHIPPER-SPEC-{prefix}"),
        );
    }
    if let Some(prefix) = target.strip_prefix("SHIPPER-ADR-") {
        return artifact_with_id_exists(
            workspace_root,
            "docs/adr",
            &format!("SHIPPER-ADR-{prefix}"),
        );
    }

    true
}

fn artifact_with_id_exists(workspace_root: &Path, dir_rel: &str, id: &str) -> bool {
    let Ok(entries) = fs::read_dir(workspace_root.join(dir_rel)) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(id) && name.ends_with(".md"))
    })
}

fn split_link_targets(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|target| target.trim().trim_matches('`').to_string())
        .filter(|target| !target.is_empty())
        .collect()
}

fn parse_headers(content: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    let mut saw_title = false;
    for line in content.lines() {
        if !saw_title {
            if line.starts_with("# ") {
                saw_title = true;
            }
            continue;
        }
        if line.starts_with("## ") {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            headers.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    headers
}

fn filename_id(rel_path: &str, prefix: &str) -> Option<String> {
    let filename = Path::new(rel_path).file_stem()?.to_str()?;
    if !filename.starts_with(prefix) {
        return None;
    }
    let suffix = filename.strip_prefix(prefix)?;
    let number = suffix.split('-').next()?;
    if number.len() != 4 || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{prefix}{number}"))
}

fn is_valid_status(status: &str) -> bool {
    matches!(
        status,
        "proposed" | "accepted" | "implemented" | "superseded"
    )
}

fn collect_documents(workspace_root: &Path) -> Result<Vec<Document>> {
    let mut documents = Vec::new();
    collect_prefixed_documents(
        workspace_root,
        "docs/proposals",
        DocumentKind::Proposal,
        "SHIPPER-PROP-",
        &mut documents,
    )?;
    collect_prefixed_documents(
        workspace_root,
        "docs/specs",
        DocumentKind::Spec,
        "SHIPPER-SPEC-",
        &mut documents,
    )?;
    collect_prefixed_documents(
        workspace_root,
        "docs/adr",
        DocumentKind::Adr,
        "SHIPPER-ADR-",
        &mut documents,
    )?;
    collect_plan_documents(workspace_root, "plans", &mut documents)?;
    documents.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(documents)
}

fn collect_prefixed_documents(
    workspace_root: &Path,
    dir_rel: &str,
    kind: DocumentKind,
    prefix: &str,
    documents: &mut Vec<Document>,
) -> Result<()> {
    let dir = workspace_root.join(dir_rel);
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if filename.starts_with(prefix) && filename.ends_with(".md") {
            push_document(workspace_root, path, kind, documents)?;
        }
    }
    Ok(())
}

fn collect_plan_documents(
    workspace_root: &Path,
    dir_rel: &str,
    documents: &mut Vec<Document>,
) -> Result<()> {
    let dir = workspace_root.join(dir_rel);
    if !dir.exists() {
        return Ok(());
    }
    collect_plan_documents_inner(workspace_root, &dir, documents)
}

fn collect_plan_documents_inner(
    workspace_root: &Path,
    dir: &Path,
    documents: &mut Vec<Document>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_plan_documents_inner(workspace_root, &path, documents)?;
            continue;
        }
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if filename.ends_with(".md") && !matches!(filename, "README.md" | "TEMPLATE.md") {
            push_document(workspace_root, path, DocumentKind::Plan, documents)?;
        }
    }
    Ok(())
}

fn push_document(
    workspace_root: &Path,
    path: PathBuf,
    kind: DocumentKind,
    documents: &mut Vec<Document>,
) -> Result<()> {
    let content =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    documents.push(Document {
        kind,
        rel_path: rel_path(workspace_root, &path)?,
        content,
    });
    Ok(())
}

fn rel_path(workspace_root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(workspace_root)
        .with_context(|| format!("computing relative path for {}", path.display()))?;
    Ok(rel
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Advisory => "advisory",
        Mode::BlockingAllowlist => "blocking-allowlist",
        Mode::BlockingStrict => "blocking-strict",
    }
}

fn mode_fails(mode: Mode, findings: &Findings) -> bool {
    match mode {
        Mode::Advisory => false,
        Mode::BlockingAllowlist | Mode::BlockingStrict => blocking_count(findings) > 0,
    }
}

fn blocking_count(findings: &Findings) -> usize {
    findings.missing_headers.len()
        + findings.invalid_ids.len()
        + findings.invalid_statuses.len()
        + findings.broken_links.len()
        + findings.parse_errors.len()
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set; run via `cargo xtask`")?;
    let xtask_dir = PathBuf::from(manifest_dir);
    xtask_dir
        .parent()
        .with_context(|| format!("xtask manifest dir has no parent: {}", xtask_dir.display()))
        .map(Path::to_path_buf)
}

fn today_iso() -> String {
    chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

fn write_report(workspace_root: &Path, report: &Report) -> Result<()> {
    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json = serde_json::to_string_pretty(report).context("serializing report as JSON")?;
    fs::write(out_dir.join(JSON_NAME), json).context("writing JSON report")?;
    fs::write(out_dir.join(MD_NAME), render_markdown(report)).context("writing Markdown report")?;
    Ok(())
}

fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Doc-Contracts Report\n\n");
    out.push_str(&format!(
        "Generated by `{} --mode {}` on {}.\n\n",
        report.tool, report.mode, report.generated_at
    ));

    out.push_str("## Summary\n\n");
    out.push_str(&format!("- Documents: {}\n", report.summary.documents));
    out.push_str(&format!(
        "- Active work items: {}\n",
        report.summary.active_work_items
    ));
    out.push_str(&format!(
        "- Missing header groups: {}\n",
        report.summary.missing_headers
    ));
    out.push_str(&format!("- Invalid IDs: {}\n", report.summary.invalid_ids));
    out.push_str(&format!(
        "- Invalid statuses: {}\n",
        report.summary.invalid_statuses
    ));
    out.push_str(&format!(
        "- Broken links: {}\n",
        report.summary.broken_links
    ));
    out.push_str(&format!(
        "- Parse errors: {}\n\n",
        report.summary.parse_errors
    ));

    section(
        &mut out,
        "Missing headers",
        &report.findings.missing_headers,
    );
    section(&mut out, "Invalid IDs", &report.findings.invalid_ids);
    section(
        &mut out,
        "Invalid statuses",
        &report.findings.invalid_statuses,
    );
    section(&mut out, "Broken links", &report.findings.broken_links);
    section(&mut out, "Parse errors", &report.findings.parse_errors);

    out
}

fn section<T: Serialize>(out: &mut String, title: &str, items: &[T]) {
    out.push_str(&format!("## {} ({})\n\n", title, items.len()));
    if items.is_empty() {
        out.push_str("_(none)_\n\n");
        return;
    }

    for item in items {
        let value = serde_json::to_value(item).unwrap_or_default();
        out.push_str(&format!("- `{}`\n", value));
    }
    out.push('\n');
}

fn print_stdout_summary(report: &Report) {
    println!(
        "doc-contracts ({}): documents={} active_work_items={} missing_headers={} invalid_ids={} invalid_statuses={} broken_links={} parse_errors={}",
        report.mode,
        report.summary.documents,
        report.summary.active_work_items,
        report.summary.missing_headers,
        report.summary.invalid_ids,
        report.summary.invalid_statuses,
        report.summary.broken_links,
        report.summary.parse_errors,
    );
}
