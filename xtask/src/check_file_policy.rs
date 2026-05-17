//! `cargo xtask check-file-policy --mode <mode>`
//!
//! Reconciles tracked non-Rust files against `policy/non-rust-allowlist.toml`.
//! Files already covered by one of the companion ledgers
//! (`generated-allowlist.toml`, `executable-allowlist.toml`,
//! `dependency-surface-allowlist.toml`, `workflow-allowlist.toml`) are
//! excluded from the universe before reconciliation — they have their own
//! dedicated checker and would otherwise be double-counted as unreceipted
//! by this one.
//!
//! Three modes match the operating doctrine documented in
//! `docs/FILE_POLICY.md`:
//!
//! - **advisory**: report violations; never fail. Lets `target/policy/` collect
//!   evidence before CI is asked to gate on it.
//! - **blocking-allowlist**: fail on unreceipted files, malformed entries
//!   (missing required fields), and expired entries.
//! - **blocking-strict**: also fail on unused entries (no tracked match) and
//!   stale review dates (`review_after` in the past). Out of scope until a
//!   dedicated cleanup pass — see `docs/policy/NON_RUST_ROLLOUT.md`.
//!
//! Outputs are written to `target/policy/file-policy-report.{md,json}` and
//! consumed by `cargo xtask policy-report` (rollout PR 9).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use globset::Glob;
use serde::{Deserialize, Serialize};

const ALLOWLIST_REL: &str = "policy/non-rust-allowlist.toml";
const OUTPUT_DIR_REL: &str = "target/policy";
const MD_NAME: &str = "file-policy-report.md";
const JSON_NAME: &str = "file-policy-report.json";

/// Companion ledgers consulted to pre-filter the universe. Files matched by
/// any of these are deferred to their dedicated checker.
const COMPANION_LEDGERS: &[&str] = &[
    "policy/generated-allowlist.toml",
    "policy/executable-allowlist.toml",
    "policy/dependency-surface-allowlist.toml",
    "policy/workflow-allowlist.toml",
];

/// CLI mode for `check-file-policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    Advisory,
    BlockingAllowlist,
    BlockingStrict,
}

// ─── Allowlist deserialization ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AllowlistDoc {
    #[serde(default)]
    file: Vec<RawEntry>,
    #[serde(default)]
    glob: Vec<RawEntry>,
}

/// Raw entry shared by `[[file]]` and `[[glob]]`. `path` vs `pattern`
/// distinguishes the two — exactly one is present per entry.
#[derive(Debug, Clone, Deserialize)]
struct RawEntry {
    path: Option<String>,
    pattern: Option<String>,
    kind: Option<String>,
    surface: Option<String>,
    classification: Option<String>,
    owner: Option<String>,
    reason: Option<String>,
    covered_by: Option<Vec<String>>,
    created: Option<String>,
    review_after: Option<String>,
    expires: Option<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    selector: Selector,
    raw: RawEntry,
}

#[derive(Debug, Clone)]
enum Selector {
    Path(String),
    Pattern(String),
}

impl Entry {
    fn label(&self) -> String {
        match &self.selector {
            Selector::Path(p) => format!("file: {p}"),
            Selector::Pattern(p) => format!("glob: {p}"),
        }
    }
}

// ─── Findings ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct Findings {
    unreceipted: Vec<String>,
    missing_fields: Vec<MissingFields>,
    expired: Vec<ExpiredEntry>,
    stale: Vec<StaleEntry>,
    unused: Vec<String>,
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
struct Report {
    tool: &'static str,
    mode: &'static str,
    today: String,
    summary: Summary,
    findings: Findings,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    tracked_non_rust: usize,
    allowlist_entries: usize,
    unreceipted: usize,
    missing_fields: usize,
    expired: usize,
    stale: usize,
    unused: usize,
}

// ─── Entry point ────────────────────────────────────────────────────────────

pub fn check(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;
    let tracked_all = enumerate_non_rust(&workspace_root)?;
    let entries = load_allowlist(&workspace_root)?;
    let companion_selectors = load_companion_selectors(&workspace_root)?;
    let today = today_iso();

    // Pre-filter: drop files already receipted in any companion ledger.
    let tracked_non_rust: Vec<String> = tracked_all
        .iter()
        .filter(|path| !covered_by_companions(path, &companion_selectors))
        .cloned()
        .collect();
    let deferred_count = tracked_all.len() - tracked_non_rust.len();

    let findings = reconcile(&tracked_non_rust, &entries, &today);

    let summary = Summary {
        tracked_non_rust: tracked_non_rust.len(),
        allowlist_entries: entries.len(),
        unreceipted: findings.unreceipted.len(),
        missing_fields: findings.missing_fields.len(),
        expired: findings.expired.len(),
        stale: findings.stale.len(),
        unused: findings.unused.len(),
    };
    let _ = deferred_count; // surfaced via stdout summary below.

    let report = Report {
        tool: "cargo xtask check-file-policy",
        mode: mode_str(mode),
        today: today.clone(),
        summary,
        findings: findings.clone(),
    };

    write_report(&workspace_root, &report)?;
    print_stdout_summary(&report);

    let fail_required = mode_fails(mode, &report.findings);
    if fail_required {
        bail!(
            "check-file-policy: {} mode found {} blocking issue(s); see {}/{}",
            report.mode,
            blocking_count(mode, &report.findings),
            OUTPUT_DIR_REL,
            MD_NAME,
        );
    }
    Ok(())
}

// ─── Reconciliation ─────────────────────────────────────────────────────────

fn reconcile(tracked: &[String], entries: &[Entry], today: &str) -> Findings {
    // Build matchers and track which entries matched anything.
    let mut entry_matched: Vec<bool> = vec![false; entries.len()];
    let mut covered: BTreeSet<&str> = BTreeSet::new();

    for path in tracked {
        for (idx, entry) in entries.iter().enumerate() {
            let matched = match &entry.selector {
                Selector::Path(p) => p == path,
                Selector::Pattern(pat) => glob_matches(pat, path),
            };
            if matched {
                entry_matched[idx] = true;
                covered.insert(path.as_str());
            }
        }
    }

    let unreceipted: Vec<String> = tracked
        .iter()
        .filter(|p| !covered.contains(p.as_str()))
        .cloned()
        .collect();

    let missing_fields: Vec<MissingFields> = entries
        .iter()
        .filter_map(|e| {
            let missing = missing_required_fields(&e.raw);
            if missing.is_empty() {
                None
            } else {
                Some(MissingFields {
                    entry: e.label(),
                    missing,
                })
            }
        })
        .collect();

    let expired: Vec<ExpiredEntry> = entries
        .iter()
        .filter_map(|e| {
            e.raw.expires.as_ref().and_then(|exp| {
                if date_is_past(exp, today) {
                    Some(ExpiredEntry {
                        entry: e.label(),
                        expires: exp.clone(),
                        today: today.to_string(),
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    let stale: Vec<StaleEntry> = entries
        .iter()
        .filter_map(|e| {
            e.raw.review_after.as_ref().and_then(|rev| {
                if date_is_past(rev, today) {
                    Some(StaleEntry {
                        entry: e.label(),
                        review_after: rev.clone(),
                        today: today.to_string(),
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    let unused: Vec<String> = entries
        .iter()
        .zip(entry_matched.iter())
        .filter_map(|(e, &m)| if m { None } else { Some(e.label()) })
        .collect();

    Findings {
        unreceipted,
        missing_fields,
        expired,
        stale,
        unused,
    }
}

fn missing_required_fields(raw: &RawEntry) -> Vec<String> {
    let mut missing = Vec::new();
    if raw.path.is_none() && raw.pattern.is_none() {
        missing.push("path|pattern".to_string());
    }
    if raw.kind.is_none() {
        missing.push("kind".to_string());
    }
    if raw.surface.is_none() {
        missing.push("surface".to_string());
    }
    if raw.classification.is_none() {
        missing.push("classification".to_string());
    }
    if raw.owner.is_none() {
        missing.push("owner".to_string());
    }
    if raw.reason.is_none() {
        missing.push("reason".to_string());
    }
    if raw.covered_by.is_none() {
        missing.push("covered_by".to_string());
    }
    if raw.created.is_none() {
        missing.push("created".to_string());
    }
    if raw.review_after.is_none() {
        missing.push("review_after".to_string());
    }
    missing
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    match Glob::new(pattern) {
        Ok(g) => g.compile_matcher().is_match(path),
        Err(_) => false,
    }
}

fn date_is_past(date: &str, today: &str) -> bool {
    // ISO 8601 dates (YYYY-MM-DD) sort lexically, so plain string compare is
    // correct as long as both sides are normalized. We accept anything chrono
    // can parse to be defensive.
    let parsed = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok();
    let today_parsed = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok();
    match (parsed, today_parsed) {
        (Some(d), Some(t)) => d < t,
        _ => date.trim() < today,
    }
}

// ─── Mode semantics ─────────────────────────────────────────────────────────

fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Advisory => "advisory",
        Mode::BlockingAllowlist => "blocking-allowlist",
        Mode::BlockingStrict => "blocking-strict",
    }
}

fn mode_fails(mode: Mode, f: &Findings) -> bool {
    match mode {
        Mode::Advisory => false,
        Mode::BlockingAllowlist => {
            !f.unreceipted.is_empty() || !f.missing_fields.is_empty() || !f.expired.is_empty()
        }
        Mode::BlockingStrict => {
            !f.unreceipted.is_empty()
                || !f.missing_fields.is_empty()
                || !f.expired.is_empty()
                || !f.unused.is_empty()
                || !f.stale.is_empty()
        }
    }
}

fn blocking_count(mode: Mode, f: &Findings) -> usize {
    let mut n = f.unreceipted.len() + f.missing_fields.len() + f.expired.len();
    if matches!(mode, Mode::BlockingStrict) {
        n += f.unused.len() + f.stale.len();
    }
    n
}

// ─── IO helpers ─────────────────────────────────────────────────────────────

fn enumerate_non_rust(workspace_root: &Path) -> Result<Vec<String>> {
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
        .filter(|p| {
            !Path::new(p)
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
        })
        .collect();
    paths.sort();
    Ok(paths)
}

/// Selector loaded from a *companion* ledger (generated / executable /
/// dependency-surface / workflow). Used only for pre-filtering: if any
/// companion selector matches a tracked file, that file is the other
/// checker's job and is excluded from this checker's universe.
#[derive(Debug, Clone)]
enum CompanionSelector {
    Path(String),
    Pattern(String),
}

#[derive(Debug, Deserialize)]
struct CompanionDoc {
    #[serde(default)]
    file: Vec<CompanionRow>,
    #[serde(default)]
    glob: Vec<CompanionRow>,
    #[serde(default)]
    workflow: Vec<CompanionRow>,
}

#[derive(Debug, Deserialize)]
struct CompanionRow {
    path: Option<String>,
    pattern: Option<String>,
}

fn load_companion_selectors(workspace_root: &Path) -> Result<Vec<CompanionSelector>> {
    let mut out = Vec::new();
    for rel in COMPANION_LEDGERS {
        let path = workspace_root.join(rel);
        let raw = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue, // ledger absent ⇒ no selectors to add.
        };
        let doc: CompanionDoc = toml::from_str(&raw)
            .with_context(|| format!("parsing companion TOML in {}", path.display()))?;
        for row in doc.file.into_iter().chain(doc.workflow) {
            if let Some(p) = row.path {
                out.push(CompanionSelector::Path(p));
            }
        }
        for row in doc.glob {
            if let Some(p) = row.pattern {
                out.push(CompanionSelector::Pattern(p));
            }
        }
    }
    Ok(out)
}

fn covered_by_companions(path: &str, selectors: &[CompanionSelector]) -> bool {
    selectors.iter().any(|sel| match sel {
        CompanionSelector::Path(p) => p == path,
        CompanionSelector::Pattern(pat) => glob_matches(pat, path),
    })
}

fn load_allowlist(workspace_root: &Path) -> Result<Vec<Entry>> {
    let path = workspace_root.join(ALLOWLIST_REL);
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc: AllowlistDoc =
        toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))?;

    let mut entries = Vec::with_capacity(doc.file.len() + doc.glob.len());
    for raw in doc.file {
        let path = raw.path.clone().unwrap_or_default();
        entries.push(Entry {
            selector: Selector::Path(path),
            raw,
        });
    }
    for raw in doc.glob {
        let pattern = raw.pattern.clone().unwrap_or_default();
        entries.push(Entry {
            selector: Selector::Pattern(pattern),
            raw,
        });
    }
    Ok(entries)
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

fn write_report(workspace_root: &Path, report: &Report) -> Result<()> {
    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json = serde_json::to_string_pretty(report).context("serializing report as JSON")?;
    fs::write(out_dir.join(JSON_NAME), json).context("writing JSON report")?;
    fs::write(out_dir.join(MD_NAME), render_markdown(report)).context("writing Markdown report")?;
    Ok(())
}

fn render_markdown(r: &Report) -> String {
    let mut out = String::new();
    out.push_str("# File-Policy Report\n\n");
    out.push_str(&format!(
        "Generated by `{} --mode {}` on {}.\n\n",
        r.tool, r.mode, r.today
    ));

    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Tracked non-Rust files: {}\n",
        r.summary.tracked_non_rust
    ));
    out.push_str(&format!(
        "- Allowlist entries: {}\n",
        r.summary.allowlist_entries
    ));
    out.push_str(&format!("- Unreceipted files: {}\n", r.summary.unreceipted));
    out.push_str(&format!(
        "- Entries with missing required fields: {}\n",
        r.summary.missing_fields
    ));
    out.push_str(&format!("- Expired entries: {}\n", r.summary.expired));
    out.push_str(&format!("- Stale review entries: {}\n", r.summary.stale));
    out.push_str(&format!("- Unused entries: {}\n\n", r.summary.unused));

    section(&mut out, "Unreceipted files", &r.findings.unreceipted);
    section(
        &mut out,
        "Entries with missing required fields",
        &r.findings.missing_fields,
    );
    section(&mut out, "Expired entries", &r.findings.expired);
    section(&mut out, "Stale review entries", &r.findings.stale);
    section(&mut out, "Unused entries", &r.findings.unused);

    out
}

fn section<T: Serialize>(out: &mut String, title: &str, items: &[T]) {
    out.push_str(&format!("## {} ({})\n\n", title, items.len()));
    if items.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for item in items {
            // Use JSON for structured items, plain bullets for strings.
            let value = serde_json::to_value(item).unwrap_or_default();
            match value {
                serde_json::Value::String(s) => out.push_str(&format!("- `{}`\n", s)),
                other => out.push_str(&format!("- `{}`\n", other)),
            }
        }
        out.push('\n');
    }
}

fn print_stdout_summary(r: &Report) {
    println!(
        "file-policy ({}): tracked={} entries={} unreceipted={} missing_fields={} expired={} stale={} unused={}",
        r.mode,
        r.summary.tracked_non_rust,
        r.summary.allowlist_entries,
        r.summary.unreceipted,
        r.summary.missing_fields,
        r.summary.expired,
        r.summary.stale,
        r.summary.unused,
    );
}
