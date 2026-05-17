//! Companion policy checks:
//!
//! - `cargo xtask check-generated`           — validates `policy/generated-allowlist.toml`.
//! - `cargo xtask check-executable-files`    — git executable bit vs `policy/executable-allowlist.toml`.
//! - `cargo xtask check-dependency-surfaces` — dep manifests/lockfiles vs `policy/dependency-surface-allowlist.toml`.
//!
//! Each check shares the same finding model as `check-file-policy`
//! (unreceipted / missing-fields / expired / stale / unused) and the same
//! advisory|blocking-allowlist|blocking-strict modes. Reports land under
//! `target/policy/<check>-report.{md,json}` and feed PR 9's unified
//! `policy-report`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use globset::Glob;
use serde::{Deserialize, Serialize};

const OUTPUT_DIR_REL: &str = "target/policy";

/// CLI mode shared by all three checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    Advisory,
    BlockingAllowlist,
    BlockingStrict,
}

// ─── Shared allowlist deserialization ───────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AllowlistDoc {
    #[serde(default)]
    file: Vec<RawEntry>,
    #[serde(default)]
    glob: Vec<RawEntry>,
}

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
    // Generated-allowlist extras.
    generator: Option<String>,
    regen_command: Option<String>,
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
struct Summary {
    universe_size: usize,
    universe_label: &'static str,
    allowlist_entries: usize,
    unreceipted: usize,
    missing_fields: usize,
    expired: usize,
    stale: usize,
    unused: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Report {
    tool: &'static str,
    mode: &'static str,
    today: String,
    summary: Summary,
    findings: Findings,
}

// ─── Public entry points (one per command) ──────────────────────────────────

pub fn check_generated(mode: Mode) -> Result<()> {
    run_check(
        mode,
        "cargo xtask check-generated",
        "policy/generated-allowlist.toml",
        "generated",
        UniverseSource::EntriesOnly,
        RequiredFieldSet::Generated,
        "generated-policy-report",
    )
}

pub fn check_executable_files(mode: Mode) -> Result<()> {
    run_check(
        mode,
        "cargo xtask check-executable-files",
        "policy/executable-allowlist.toml",
        "tracked executable files",
        UniverseSource::ExecutableTrackedFiles,
        RequiredFieldSet::Standard,
        "executable-policy-report",
    )
}

pub fn check_dependency_surfaces(mode: Mode) -> Result<()> {
    run_check(
        mode,
        "cargo xtask check-dependency-surfaces",
        "policy/dependency-surface-allowlist.toml",
        "dependency-surface files",
        UniverseSource::DependencySurfaceFiles,
        RequiredFieldSet::Standard,
        "dependency-surface-policy-report",
    )
}

// ─── Configurable check body ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum UniverseSource {
    /// No fixed universe — only validate entries themselves. Used for
    /// `check-generated` where the set of "generated files" is defined by
    /// the allowlist itself; there is no canonical out-of-band signal.
    EntriesOnly,
    /// Universe = tracked files with the git executable bit set (`100755`).
    ExecutableTrackedFiles,
    /// Universe = tracked dependency-manifest/lockfile candidates
    /// (Cargo.toml, Cargo.lock, deny.toml, crates/*/Cargo.toml, fuzz/Cargo.toml,
    /// fuzz/Cargo.lock).
    DependencySurfaceFiles,
}

#[derive(Debug, Clone, Copy)]
enum RequiredFieldSet {
    Standard,
    Generated,
}

fn run_check(
    mode: Mode,
    tool: &'static str,
    allowlist_rel: &str,
    universe_label: &'static str,
    universe_src: UniverseSource,
    required_fields: RequiredFieldSet,
    report_basename: &str,
) -> Result<()> {
    let workspace_root = workspace_root()?;
    let entries = load_allowlist(&workspace_root, allowlist_rel)?;
    let receipt_universe = collect_receipt_universe(&workspace_root, universe_src)?;
    // For `unused` detection an entry must be able to match against every
    // tracked file, not just the scoped receipt universe — otherwise a glob
    // like `**/*.snap` looks "unused" in EntriesOnly mode because the
    // receipt universe is empty.
    let match_universe = collect_match_universe(&workspace_root, universe_src)?;
    let today = today_iso();

    let findings = reconcile(
        &receipt_universe,
        &match_universe,
        &entries,
        required_fields,
        &today,
    );

    let summary = Summary {
        universe_size: receipt_universe.len(),
        universe_label,
        allowlist_entries: entries.len(),
        unreceipted: findings.unreceipted.len(),
        missing_fields: findings.missing_fields.len(),
        expired: findings.expired.len(),
        stale: findings.stale.len(),
        unused: findings.unused.len(),
    };

    let report = Report {
        tool,
        mode: mode_str(mode),
        today: today.clone(),
        summary,
        findings: findings.clone(),
    };

    write_report(&workspace_root, report_basename, &report)?;
    print_stdout_summary(&report);

    if mode_fails(mode, &report.findings, universe_src) {
        bail!(
            "{}: {} mode found {} blocking issue(s); see {}/{}.md",
            tool,
            report.mode,
            blocking_count(mode, &report.findings, universe_src),
            OUTPUT_DIR_REL,
            report_basename,
        );
    }
    Ok(())
}

// ─── Reconciliation ─────────────────────────────────────────────────────────

fn reconcile(
    receipt_universe: &[String],
    match_universe: &[String],
    entries: &[Entry],
    required_fields: RequiredFieldSet,
    today: &str,
) -> Findings {
    // `entry_matched` is computed against the full match universe so that
    // patterns like `**/*.snap` register as "used" even when the receipt
    // universe is empty (EntriesOnly mode).
    let mut entry_matched: Vec<bool> = vec![false; entries.len()];
    for path in match_universe {
        for (idx, entry) in entries.iter().enumerate() {
            if entry_matches(entry, path) {
                entry_matched[idx] = true;
            }
        }
    }

    // `covered` is scoped to the receipt universe — only files that *must*
    // be receipted count for the unreceipted finding.
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    for path in receipt_universe {
        for entry in entries {
            if entry_matches(entry, path) {
                covered.insert(path.as_str());
            }
        }
    }

    let unreceipted: Vec<String> = receipt_universe
        .iter()
        .filter(|p| !covered.contains(p.as_str()))
        .cloned()
        .collect();

    let missing_fields: Vec<MissingFields> = entries
        .iter()
        .filter_map(|e| {
            let missing = missing_required_fields(&e.raw, required_fields);
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

fn missing_required_fields(raw: &RawEntry, set: RequiredFieldSet) -> Vec<String> {
    let mut missing = Vec::new();
    if raw.path.is_none() && raw.pattern.is_none() {
        missing.push("path|pattern".to_string());
    }
    for (name, present) in [
        ("kind", raw.kind.is_some()),
        ("surface", raw.surface.is_some()),
        ("classification", raw.classification.is_some()),
        ("owner", raw.owner.is_some()),
        ("reason", raw.reason.is_some()),
        ("covered_by", raw.covered_by.is_some()),
        ("created", raw.created.is_some()),
        ("review_after", raw.review_after.is_some()),
    ] {
        if !present {
            missing.push(name.to_string());
        }
    }
    if let RequiredFieldSet::Generated = set {
        if raw.generator.is_none() {
            missing.push("generator".to_string());
        }
        if raw.regen_command.is_none() {
            missing.push("regen_command".to_string());
        }
    }
    missing
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    match Glob::new(pattern) {
        Ok(g) => g.compile_matcher().is_match(path),
        Err(_) => false,
    }
}

fn entry_matches(entry: &Entry, path: &str) -> bool {
    match &entry.selector {
        Selector::Path(p) => p == path,
        Selector::Pattern(pat) => glob_matches(pat, path),
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

// ─── Mode semantics ─────────────────────────────────────────────────────────

fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Advisory => "advisory",
        Mode::BlockingAllowlist => "blocking-allowlist",
        Mode::BlockingStrict => "blocking-strict",
    }
}

fn mode_fails(mode: Mode, f: &Findings, src: UniverseSource) -> bool {
    let unreceipted_meaningful = !matches!(src, UniverseSource::EntriesOnly);
    match mode {
        Mode::Advisory => false,
        Mode::BlockingAllowlist => {
            (unreceipted_meaningful && !f.unreceipted.is_empty())
                || !f.missing_fields.is_empty()
                || !f.expired.is_empty()
        }
        Mode::BlockingStrict => {
            (unreceipted_meaningful && !f.unreceipted.is_empty())
                || !f.missing_fields.is_empty()
                || !f.expired.is_empty()
                || !f.unused.is_empty()
                || !f.stale.is_empty()
        }
    }
}

fn blocking_count(mode: Mode, f: &Findings, src: UniverseSource) -> usize {
    let unreceipted_meaningful = !matches!(src, UniverseSource::EntriesOnly);
    let mut n = f.missing_fields.len() + f.expired.len();
    if unreceipted_meaningful {
        n += f.unreceipted.len();
    }
    if matches!(mode, Mode::BlockingStrict) {
        n += f.unused.len() + f.stale.len();
    }
    n
}

// ─── Universe collection ────────────────────────────────────────────────────

/// Files that *must* be receipted (used for the unreceipted finding).
fn collect_receipt_universe(workspace_root: &Path, src: UniverseSource) -> Result<Vec<String>> {
    match src {
        UniverseSource::EntriesOnly => Ok(Vec::new()),
        UniverseSource::ExecutableTrackedFiles => executable_tracked_files(workspace_root),
        UniverseSource::DependencySurfaceFiles => dependency_surface_files(workspace_root),
    }
}

/// Files an entry is allowed to match against (used for the unused finding).
/// For EntriesOnly checks we still want patterns like `**/*.snap` to count
/// as matched if real tracked files satisfy them.
fn collect_match_universe(workspace_root: &Path, src: UniverseSource) -> Result<Vec<String>> {
    match src {
        UniverseSource::EntriesOnly => all_tracked_files(workspace_root),
        UniverseSource::ExecutableTrackedFiles => executable_tracked_files(workspace_root),
        UniverseSource::DependencySurfaceFiles => dependency_surface_files(workspace_root),
    }
}

fn all_tracked_files(workspace_root: &Path) -> Result<Vec<String>> {
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
        .collect();
    paths.sort();
    Ok(paths)
}

/// `git ls-files --stage -z` parses to lines of `<mode> <sha> <stage>\t<path>`.
/// We keep only entries with mode `100755` (regular executable).
fn executable_tracked_files(workspace_root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("ls-files")
        .arg("--stage")
        .arg("-z")
        .output()
        .context("running `git ls-files --stage -z`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`git ls-files --stage -z` exited {}: {}",
            output.status,
            stderr.trim()
        );
    }
    let mut paths: Vec<String> = output
        .stdout
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|bytes| {
            let line = String::from_utf8_lossy(bytes);
            // Expect: <mode> <sha> <stage>\t<path>
            let (meta, path) = line.split_once('\t')?;
            let mode = meta.split_whitespace().next()?;
            if mode == "100755" {
                Some(path.to_string())
            } else {
                None
            }
        })
        .collect();
    paths.sort();
    Ok(paths)
}

/// Dependency-surface universe: top-level Cargo.toml, Cargo.lock, deny.toml,
/// plus every per-crate Cargo.toml, plus the fuzz crate's manifests.
fn dependency_surface_files(workspace_root: &Path) -> Result<Vec<String>> {
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
    let crates_glob = Glob::new("crates/*/Cargo.toml")?.compile_matcher();
    let fuzz_glob = Glob::new("fuzz/Cargo.*")?.compile_matcher();

    let mut paths: Vec<String> = output
        .stdout
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .filter(|p| {
            matches!(p.as_str(), "Cargo.toml" | "Cargo.lock" | "deny.toml")
                || crates_glob.is_match(p)
                || fuzz_glob.is_match(p)
        })
        .collect();
    paths.sort();
    Ok(paths)
}

// ─── IO helpers ─────────────────────────────────────────────────────────────

fn load_allowlist(workspace_root: &Path, rel: &str) -> Result<Vec<Entry>> {
    let path = workspace_root.join(rel);
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc: AllowlistDoc =
        toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))?;

    let mut entries = Vec::with_capacity(doc.file.len() + doc.glob.len());
    for raw in doc.file {
        let p = raw.path.clone().unwrap_or_default();
        entries.push(Entry {
            selector: Selector::Path(p),
            raw,
        });
    }
    for raw in doc.glob {
        let p = raw.pattern.clone().unwrap_or_default();
        entries.push(Entry {
            selector: Selector::Pattern(p),
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

fn write_report(workspace_root: &Path, basename: &str, report: &Report) -> Result<()> {
    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let json = serde_json::to_string_pretty(report).context("serializing report as JSON")?;
    fs::write(out_dir.join(format!("{basename}.json")), json).context("writing JSON report")?;
    fs::write(
        out_dir.join(format!("{basename}.md")),
        render_markdown(report),
    )
    .context("writing Markdown report")?;
    Ok(())
}

fn render_markdown(r: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} Report\n\n", r.tool));
    out.push_str(&format!(
        "Generated by `{} --mode {}` on {}.\n\n",
        r.tool, r.mode, r.today
    ));

    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- {}: {}\n",
        r.summary.universe_label, r.summary.universe_size
    ));
    out.push_str(&format!(
        "- Allowlist entries: {}\n",
        r.summary.allowlist_entries
    ));
    out.push_str(&format!("- Unreceipted: {}\n", r.summary.unreceipted));
    out.push_str(&format!(
        "- Entries with missing required fields: {}\n",
        r.summary.missing_fields
    ));
    out.push_str(&format!("- Expired entries: {}\n", r.summary.expired));
    out.push_str(&format!("- Stale review entries: {}\n", r.summary.stale));
    out.push_str(&format!("- Unused entries: {}\n\n", r.summary.unused));

    section_strings(&mut out, "Unreceipted", &r.findings.unreceipted);
    section_missing(&mut out, &r.findings.missing_fields);
    section_expired(&mut out, &r.findings.expired);
    section_stale(&mut out, &r.findings.stale);
    section_strings(&mut out, "Unused entries", &r.findings.unused);

    out
}

fn section_strings(out: &mut String, title: &str, items: &[String]) {
    out.push_str(&format!("## {} ({})\n\n", title, items.len()));
    if items.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for s in items {
            out.push_str(&format!("- `{}`\n", s));
        }
        out.push('\n');
    }
}

fn section_missing(out: &mut String, items: &[MissingFields]) {
    out.push_str(&format!(
        "## Entries with missing required fields ({})\n\n",
        items.len()
    ));
    if items.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for m in items {
            out.push_str(&format!("- `{}`: {}\n", m.entry, m.missing.join(", ")));
        }
        out.push('\n');
    }
}

fn section_expired(out: &mut String, items: &[ExpiredEntry]) {
    out.push_str(&format!("## Expired entries ({})\n\n", items.len()));
    if items.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for e in items {
            out.push_str(&format!(
                "- `{}` (expires={}, today={})\n",
                e.entry, e.expires, e.today
            ));
        }
        out.push('\n');
    }
}

fn section_stale(out: &mut String, items: &[StaleEntry]) {
    out.push_str(&format!("## Stale review entries ({})\n\n", items.len()));
    if items.is_empty() {
        out.push_str("_(none)_\n\n");
    } else {
        for s in items {
            out.push_str(&format!(
                "- `{}` (review_after={}, today={})\n",
                s.entry, s.review_after, s.today
            ));
        }
        out.push('\n');
    }
}

fn print_stdout_summary(r: &Report) {
    println!(
        "{} ({}): {}={} entries={} unreceipted={} missing_fields={} expired={} stale={} unused={}",
        r.tool,
        r.mode,
        r.summary.universe_label,
        r.summary.universe_size,
        r.summary.allowlist_entries,
        r.summary.unreceipted,
        r.summary.missing_fields,
        r.summary.expired,
        r.summary.stale,
        r.summary.unused,
    );
}
