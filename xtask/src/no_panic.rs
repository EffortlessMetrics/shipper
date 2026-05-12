//! `cargo xtask no-panic baseline` — AST-based panic-family detector (#187).
//!
//! Walks every tracked production-source Rust file in the workspace
//! (`crates/*/src/**/*.rs`, excluding `tests/`, `benches/`, `examples/`,
//! and `#[cfg(test)]`/`#[test]` subtrees), classifies every panic-family
//! call site by exact identity, groups by (path, family, selector_kind,
//! selector_callee, snippet), and writes the count-keyed result to
//! `policy/no-panic-baseline.json`.
//!
//! Detection is AST-based (via `syn`) rather than regex-based so chained
//! calls like `.lock().unwrap()`, macro invocations, and cfg-gated test
//! blocks are classified exactly. See docs/NO_PANIC_POLICY.md for the
//! intended semantics; this module is the operational implementation.
//!
//! This PR ships the detector + baseline only — the matching `check`
//! subcommand (verify mode + release CI gate) lands in PR 8b.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use proc_macro2::LineColumn;
use serde::{Deserialize, Serialize};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, ExprIndex, ExprMacro, ExprMethodCall, File, ImplItemFn, ItemFn, ItemImpl, ItemMod,
    TraitItemFn,
};

const BASELINE_PATH: &str = "policy/no-panic-baseline.json";
const SCHEMA_VERSION: &str = "1.0";
const SNIPPET_MAX_LEN: usize = 120;

/// One finding the detector recognized, before grouping.
#[derive(Debug, Clone)]
struct Finding {
    path: String,
    line: usize,
    family: &'static str,
    selector_kind: &'static str,
    selector_callee: String,
    snippet: String,
    test_code: bool,
}

/// Grouping key. Matches the identity tuple documented in NO_PANIC_POLICY.md.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct GroupKey {
    path: String,
    family: &'static str,
    selector_kind: &'static str,
    selector_callee: String,
    snippet: String,
}

#[derive(Debug, Serialize)]
struct BaselineFile {
    schema_version: &'static str,
    generated_by: &'static str,
    generated_on: String,
    note: &'static str,
    counts: Counts,
    entries: Vec<BaselineEntry>,
}

#[derive(Debug, Serialize)]
struct Counts {
    total_call_sites: u64,
    total_entries: u64,
    files_scanned: u64,
    files_with_findings: u64,
    by_family: BTreeMap<&'static str, u64>,
}

#[derive(Debug, Serialize)]
struct BaselineEntry {
    path: String,
    family: &'static str,
    selector_kind: &'static str,
    selector_callee: String,
    snippet: String,
    count: u64,
    first_line: usize,
}

/// Deserialized view of an existing `policy/no-panic-baseline.json` entry.
/// Family/selector_kind are owned `String`s here (versus `&'static str` in
/// the freshly-scanned `BaselineEntry`) because they come from disk.
#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
struct BaselineEntryOnDisk {
    path: String,
    family: String,
    selector_kind: String,
    selector_callee: String,
    snippet: String,
    count: u64,
    #[serde(default)]
    first_line: usize,
}

#[derive(Debug, Deserialize)]
struct BaselineFileOnDisk {
    #[allow(dead_code)]
    schema_version: String,
    #[serde(default)]
    entries: Vec<BaselineEntryOnDisk>,
}

/// Identity key shared between disk entries and fresh entries.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct EntryKey {
    path: String,
    family: String,
    selector_kind: String,
    selector_callee: String,
    snippet: String,
}

impl EntryKey {
    fn from_disk(e: &BaselineEntryOnDisk) -> Self {
        Self {
            path: e.path.clone(),
            family: e.family.clone(),
            selector_kind: e.selector_kind.clone(),
            selector_callee: e.selector_callee.clone(),
            snippet: e.snippet.clone(),
        }
    }
    fn from_fresh(e: &BaselineEntry) -> Self {
        Self {
            path: e.path.clone(),
            family: e.family.to_string(),
            selector_kind: e.selector_kind.to_string(),
            selector_callee: e.selector_callee.clone(),
            snippet: e.snippet.clone(),
        }
    }
}

/// Scan + group result. Owned by `baseline()` (writes to disk) and `check()`
/// (compares to disk). Both call the same scanner so the production-vs-test
/// classification is identical at baseline-time and check-time.
struct ScanResult {
    entries: Vec<BaselineEntry>,
    counts: Counts,
    dropped_test_sites: u64,
}

fn scan_and_group(workspace_root: &Path) -> Result<ScanResult> {
    let files = enumerate_source_files(workspace_root)?;
    let total_files = files.len() as u64;

    let mut all_findings: Vec<Finding> = Vec::new();
    let mut files_with_findings: u64 = 0;

    for rel in &files {
        let abs = workspace_root.join(rel);
        let source = match fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: could not read {}: {e}", rel.display());
                continue;
            }
        };
        let parsed: File = match syn::parse_file(&source) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("warning: syn parse error in {}: {e}", rel.display());
                continue;
            }
        };
        let source_lines: Vec<&str> = source.lines().collect();
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let mut visitor = PanicVisitor::new(rel_str.clone(), &source_lines);
        visitor.visit_file(&parsed);
        if !visitor.findings.is_empty() {
            files_with_findings += 1;
        }
        all_findings.extend(visitor.findings);
    }

    // Test-code findings (inside `#[cfg(test)]` or `#[test]` subtrees) are
    // excluded from the production baseline per docs/NO_PANIC_POLICY.md.
    // `**/tests.rs`, `**/_tests.rs`, and `tests/`/`benches/`/`examples/`
    // dirs are already excluded at enumeration time; this drops findings
    // that hid inside production source files.
    let production_findings: Vec<&Finding> = all_findings.iter().filter(|f| !f.test_code).collect();
    let dropped_test_sites = (all_findings.len() - production_findings.len()) as u64;

    let mut by_family: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut groups: BTreeMap<GroupKey, (u64, usize)> = BTreeMap::new();
    let total_sites = production_findings.len() as u64;
    for f in &production_findings {
        *by_family.entry(f.family).or_default() += 1;
        let key = GroupKey {
            path: f.path.clone(),
            family: f.family,
            selector_kind: f.selector_kind,
            selector_callee: f.selector_callee.clone(),
            snippet: f.snippet.clone(),
        };
        let entry = groups.entry(key).or_insert((0, f.line));
        entry.0 += 1;
        if f.line < entry.1 {
            entry.1 = f.line;
        }
    }

    let entries: Vec<BaselineEntry> = groups
        .into_iter()
        .map(|(k, (count, first_line))| BaselineEntry {
            path: k.path,
            family: k.family,
            selector_kind: k.selector_kind,
            selector_callee: k.selector_callee,
            snippet: k.snippet,
            count,
            first_line,
        })
        .collect();

    Ok(ScanResult {
        counts: Counts {
            total_call_sites: total_sites,
            total_entries: entries.len() as u64,
            files_scanned: total_files,
            files_with_findings,
            by_family,
        },
        entries,
        dropped_test_sites,
    })
}

pub fn baseline() -> Result<()> {
    let workspace_root = workspace_root()?;
    let scan = scan_and_group(&workspace_root)?;

    let baseline = BaselineFile {
        schema_version: SCHEMA_VERSION,
        generated_by: "cargo xtask no-panic baseline (#187)",
        generated_on: chrono::Utc::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
        note: "Machine-generated. Run `cargo xtask no-panic baseline` to regenerate. \
               See docs/NO_PANIC_POLICY.md for semantics.",
        counts: scan.counts,
        entries: scan.entries,
    };

    let out_path = workspace_root.join(BASELINE_PATH);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut json =
        serde_json::to_string_pretty(&baseline).context("serialising baseline to JSON")?;
    json.push('\n');
    fs::write(&out_path, json).with_context(|| format!("writing {}", out_path.display()))?;

    println!(
        "no-panic baseline: files_scanned={} files_with_findings={} production_sites={} entries={} (dropped {} test-code sites)",
        baseline.counts.files_scanned,
        baseline.counts.files_with_findings,
        baseline.counts.total_call_sites,
        baseline.counts.total_entries,
        scan.dropped_test_sites,
    );
    for (family, n) in &baseline.counts.by_family {
        println!("  {family}: {n}");
    }
    println!("wrote {}", BASELINE_PATH);
    Ok(())
}

/// Reporting / enforcement mode. Matches the pattern in `check_file_policy`
/// and the `checks` module: `Blocking` exits non-zero on any violation,
/// `Advisory` writes the report and returns success regardless.
#[derive(Debug, Copy, Clone, clap::ValueEnum)]
pub enum Mode {
    Advisory,
    Blocking,
}

#[derive(Debug, Serialize)]
struct CheckReport {
    tool: &'static str,
    summary: CheckSummary,
    new_entries: Vec<DiffEntry>,
    count_increases: Vec<DiffCount>,
    resolved_entries: Vec<DiffEntry>,
    count_decreases: Vec<DiffCount>,
}

#[derive(Debug, Serialize)]
struct CheckSummary {
    baseline_entries: u64,
    current_entries: u64,
    new_debt: u64,
    count_increases: u64,
    resolved: u64,
    count_decreases: u64,
    /// Headline-eligible: new entries + count increases. Surfaces as
    /// `unreceipted` in `cargo xtask policy-report` for parity with the
    /// other policy areas.
    violations: u64,
}

#[derive(Debug, Serialize, Clone)]
struct DiffEntry {
    path: String,
    family: String,
    selector_kind: String,
    selector_callee: String,
    snippet: String,
}

#[derive(Debug, Serialize, Clone)]
struct DiffCount {
    path: String,
    family: String,
    selector_callee: String,
    snippet: String,
    baseline: u64,
    current: u64,
}

const CHECK_REPORT_REL: &str = "target/policy/no-panic-report.json";

pub fn check(mode: Mode) -> Result<()> {
    let workspace_root = workspace_root()?;

    // Load on-disk baseline.
    let baseline_path = workspace_root.join(BASELINE_PATH);
    let baseline_raw = fs::read_to_string(&baseline_path)
        .with_context(|| format!("reading {}", baseline_path.display()))?;
    let on_disk: BaselineFileOnDisk = serde_json::from_str(&baseline_raw)
        .with_context(|| format!("parsing {}", baseline_path.display()))?;
    let mut disk_by_key: BTreeMap<EntryKey, u64> = BTreeMap::new();
    for e in &on_disk.entries {
        disk_by_key.insert(EntryKey::from_disk(e), e.count);
    }

    // Re-scan current source.
    let scan = scan_and_group(&workspace_root)?;
    let mut fresh_by_key: BTreeMap<EntryKey, u64> = BTreeMap::new();
    for e in &scan.entries {
        fresh_by_key.insert(EntryKey::from_fresh(e), e.count);
    }

    // Categorise differences.
    let disk_keys: BTreeSet<&EntryKey> = disk_by_key.keys().collect();
    let fresh_keys: BTreeSet<&EntryKey> = fresh_by_key.keys().collect();

    let new_debt: Vec<&EntryKey> = fresh_keys.difference(&disk_keys).copied().collect();
    let resolved: Vec<&EntryKey> = disk_keys.difference(&fresh_keys).copied().collect();
    let mut count_increases: Vec<(&EntryKey, u64, u64)> = Vec::new();
    let mut count_decreases: Vec<(&EntryKey, u64, u64)> = Vec::new();
    for key in disk_keys.intersection(&fresh_keys) {
        let disk_count = disk_by_key[*key];
        let fresh_count = fresh_by_key[*key];
        if fresh_count > disk_count {
            count_increases.push((*key, disk_count, fresh_count));
        } else if fresh_count < disk_count {
            count_decreases.push((*key, disk_count, fresh_count));
        }
    }

    let violations = (new_debt.len() + count_increases.len()) as u64;

    let report = CheckReport {
        tool: "cargo xtask no-panic check",
        summary: CheckSummary {
            baseline_entries: on_disk.entries.len() as u64,
            current_entries: scan.entries.len() as u64,
            new_debt: new_debt.len() as u64,
            count_increases: count_increases.len() as u64,
            resolved: resolved.len() as u64,
            count_decreases: count_decreases.len() as u64,
            violations,
        },
        new_entries: new_debt
            .iter()
            .map(|k| DiffEntry {
                path: k.path.clone(),
                family: k.family.clone(),
                selector_kind: k.selector_kind.clone(),
                selector_callee: k.selector_callee.clone(),
                snippet: k.snippet.clone(),
            })
            .collect(),
        count_increases: count_increases
            .iter()
            .map(|(k, b, c)| DiffCount {
                path: k.path.clone(),
                family: k.family.clone(),
                selector_callee: k.selector_callee.clone(),
                snippet: k.snippet.clone(),
                baseline: *b,
                current: *c,
            })
            .collect(),
        resolved_entries: resolved
            .iter()
            .map(|k| DiffEntry {
                path: k.path.clone(),
                family: k.family.clone(),
                selector_kind: k.selector_kind.clone(),
                selector_callee: k.selector_callee.clone(),
                snippet: k.snippet.clone(),
            })
            .collect(),
        count_decreases: count_decreases
            .iter()
            .map(|(k, b, c)| DiffCount {
                path: k.path.clone(),
                family: k.family.clone(),
                selector_callee: k.selector_callee.clone(),
                snippet: k.snippet.clone(),
                baseline: *b,
                current: *c,
            })
            .collect(),
    };

    // Always write the report — both advisory and blocking modes leave
    // an artifact CI can upload.
    let out_path = workspace_root.join(CHECK_REPORT_REL);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut json = serde_json::to_string_pretty(&report).context("serialising check report")?;
    json.push('\n');
    fs::write(&out_path, json).with_context(|| format!("writing {}", out_path.display()))?;

    println!(
        "no-panic check: baseline_entries={} current_entries={} new={} resolved={} count_increase={} count_decrease={}",
        report.summary.baseline_entries,
        report.summary.current_entries,
        report.summary.new_debt,
        report.summary.resolved,
        report.summary.count_increases,
        report.summary.count_decreases,
    );

    // Resolved / decreased entries are good news; print them so the developer
    // who reduced debt gets credit in CI logs.
    for k in &resolved {
        println!("  resolved: {}  {}  {}", k.path, k.family, k.snippet);
    }
    for (k, disk, fresh) in &count_decreases {
        println!(
            "  decreased: {}  {}  {} -> {}  {}",
            k.path, k.family, disk, fresh, k.snippet
        );
    }

    if violations == 0 {
        return Ok(());
    }

    for k in &new_debt {
        eprintln!(
            "new panic-family debt: {}  family={}  selector_kind={}  callee={}  snippet={}",
            k.path, k.family, k.selector_kind, k.selector_callee, k.snippet
        );
    }
    for (k, disk, fresh) in &count_increases {
        eprintln!(
            "count increased: {}  family={}  {} -> {}  snippet={}",
            k.path, k.family, disk, fresh, k.snippet
        );
    }
    eprintln!();
    eprintln!(
        "no-panic policy: {} new entr{} and {} count increase{} since the baseline.",
        new_debt.len(),
        if new_debt.len() == 1 { "y" } else { "ies" },
        count_increases.len(),
        if count_increases.len() == 1 { "" } else { "s" },
    );
    eprintln!(
        "if these additions are intentional (e.g. a refactor that legitimately moved \
         existing debt to a new call site), regenerate the baseline with: \
         `cargo xtask no-panic baseline`, and explain the rationale in the PR body."
    );

    match mode {
        Mode::Advisory => Ok(()),
        Mode::Blocking => bail!("no-panic check failed"),
    }
}

// ─── File enumeration ──────────────────────────────────────────────────────

fn enumerate_source_files(workspace_root: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("ls-files")
        .arg("-z")
        .arg("crates/*/src/**/*.rs")
        .output()
        .context("running `git ls-files -z crates/*/src/**/*.rs`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`git ls-files` exited {}: {}", output.status, stderr.trim());
    }
    let mut files: Vec<PathBuf> = output
        .stdout
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
        .filter(|p| !is_excluded_dir(p))
        .collect();
    files.sort();
    Ok(files)
}

fn is_excluded_dir(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    // Out of scope per docs/NO_PANIC_POLICY.md:
    //   * `tests/`, `benches/`, `examples/` dirs anywhere in the tree are
    //     test-only by Cargo convention.
    //   * `**/tests.rs` files inside `src/` are conventionally compiled
    //     only under `#[cfg(test)] mod tests;` and are entirely test code.
    //     Excluding at enumeration time catches the helper fns that the
    //     visitor's `#[test]`/`#[cfg(test)]` attribute checks would miss.
    if s.contains("/tests/") || s.contains("/benches/") || s.contains("/examples/") {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str())
        && (name == "tests.rs" || name.ends_with("_tests.rs"))
    {
        return true;
    }
    false
}

// ─── Visitor ───────────────────────────────────────────────────────────────

struct PanicVisitor<'a> {
    path: String,
    source_lines: &'a [&'a str],
    cfg_test_depth: u32,
    findings: Vec<Finding>,
}

impl<'a> PanicVisitor<'a> {
    fn new(path: String, source_lines: &'a [&'a str]) -> Self {
        Self {
            path,
            source_lines,
            cfg_test_depth: 0,
            findings: Vec::new(),
        }
    }

    fn in_test(&self) -> bool {
        self.cfg_test_depth > 0
    }

    fn snippet_for(&self, span: proc_macro2::Span) -> (usize, String) {
        let LineColumn { line, .. } = span.start();
        let raw = self
            .source_lines
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or("")
            .trim();
        let snippet = if raw.len() > SNIPPET_MAX_LEN {
            let mut s = raw.chars().take(SNIPPET_MAX_LEN).collect::<String>();
            s.push('…');
            s
        } else {
            raw.to_string()
        };
        (line, snippet)
    }

    fn push_finding(
        &mut self,
        family: &'static str,
        selector_kind: &'static str,
        selector_callee: String,
        span: proc_macro2::Span,
    ) {
        let (line, snippet) = self.snippet_for(span);
        self.findings.push(Finding {
            path: self.path.clone(),
            line,
            family,
            selector_kind,
            selector_callee,
            snippet,
            test_code: self.in_test(),
        });
    }
}

impl<'ast> Visit<'ast> for PanicVisitor<'_> {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        let push = attrs_imply_test(&item.attrs);
        if push {
            self.cfg_test_depth += 1;
        }
        visit::visit_item_mod(self, item);
        if push {
            self.cfg_test_depth -= 1;
        }
    }

    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        let push = attrs_imply_test(&item.attrs);
        if push {
            self.cfg_test_depth += 1;
        }
        visit::visit_item_fn(self, item);
        if push {
            self.cfg_test_depth -= 1;
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        let push = attrs_imply_test(&item.attrs);
        if push {
            self.cfg_test_depth += 1;
        }
        visit::visit_impl_item_fn(self, item);
        if push {
            self.cfg_test_depth -= 1;
        }
    }

    fn visit_trait_item_fn(&mut self, item: &'ast TraitItemFn) {
        let push = attrs_imply_test(&item.attrs);
        if push {
            self.cfg_test_depth += 1;
        }
        visit::visit_trait_item_fn(self, item);
        if push {
            self.cfg_test_depth -= 1;
        }
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        let push = attrs_imply_test(&item.attrs);
        if push {
            self.cfg_test_depth += 1;
        }
        visit::visit_item_impl(self, item);
        if push {
            self.cfg_test_depth -= 1;
        }
    }

    fn visit_expr_method_call(&mut self, expr: &'ast ExprMethodCall) {
        let name = expr.method.to_string();
        let family = match name.as_str() {
            "unwrap" => Some("unwrap"),
            "expect" => Some("expect"),
            _ => None,
        };
        if let Some(family) = family {
            self.push_finding(family, "method_call", name.clone(), expr.span());
        }
        visit::visit_expr_method_call(self, expr);
    }

    fn visit_expr_macro(&mut self, expr: &'ast ExprMacro) {
        if let Some(last) = expr.mac.path.segments.last() {
            let name = last.ident.to_string();
            let family: Option<&'static str> = match name.as_str() {
                "panic" | "panic_any" => Some("panic"),
                "unreachable" => Some("unreachable"),
                "todo" => Some("todo"),
                "unimplemented" => Some("unimplemented"),
                _ => None,
            };
            if let Some(family) = family {
                self.push_finding(family, "macro", name, expr.span());
            }
        }
        visit::visit_expr_macro(self, expr);
    }

    fn visit_expr_index(&mut self, expr: &'ast ExprIndex) {
        // Slice / map / Vec indexing. We cannot tell from syn alone whether
        // the indexed type implements `Index` infallibly (e.g., a `[T; N]`
        // with a const index) versus fallibly — see docs/NO_PANIC_POLICY.md
        // for the rationale for tracking all indexing.
        self.push_finding("index", "syntax", "[]".to_string(), expr.span());
        visit::visit_expr_index(self, expr);
    }

    // Skip macro bodies entirely — syn does not parse macro inputs, so any
    // `.unwrap()` inside `vec![…]` or `assert_eq!(…)` is invisible. This is
    // the documented limitation: the baseline tracks what the AST can see.
    fn visit_macro(&mut self, _mac: &'ast syn::Macro) {}
}

// ─── Attribute helpers ─────────────────────────────────────────────────────

fn attrs_imply_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(attr_implies_test)
}

fn attr_implies_test(attr: &Attribute) -> bool {
    if attr.path().is_ident("test") || attr.path().is_ident("bench") {
        return true;
    }
    if attr.path().is_ident("cfg") {
        // `parse_nested_meta` walks the meta items inside `cfg(...)`. We
        // recurse into `any(...)` and `all(...)`, but NOT into `not(...)`:
        // `cfg(not(test))` is production-only code, so a nested `test`
        // inside `not(...)` does not imply test-classification.
        let mut hit = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                hit = true;
            } else if meta.path.is_ident("any") || meta.path.is_ident("all") {
                let _ = meta.parse_nested_meta(|inner| {
                    if inner.path.is_ident("test") {
                        hit = true;
                    }
                    Ok(())
                });
            }
            Ok(())
        });
        if hit {
            return true;
        }
    }
    false
}

// ─── Workspace root ────────────────────────────────────────────────────────

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

    fn parse_attrs(src: &str) -> Vec<Attribute> {
        // Parse a synthetic item that carries `src` as its attribute(s) so
        // the parser produces real `Attribute` nodes against the same syn
        // grammar the visitor uses.
        let item: syn::Item = syn::parse_str(&format!("{src}\nfn _probe() {{}}"))
            .expect("synthetic attr probe parses");
        match item {
            syn::Item::Fn(f) => f.attrs,
            _ => panic!("probe must be a fn"),
        }
    }

    #[test]
    fn attrs_test_attribute_is_test() {
        assert!(attrs_imply_test(&parse_attrs("#[test]")));
    }

    #[test]
    fn attrs_bench_attribute_is_test() {
        assert!(attrs_imply_test(&parse_attrs("#[bench]")));
    }

    #[test]
    fn attrs_cfg_test_is_test() {
        assert!(attrs_imply_test(&parse_attrs("#[cfg(test)]")));
    }

    #[test]
    fn attrs_cfg_any_test_is_test() {
        assert!(attrs_imply_test(&parse_attrs(
            "#[cfg(any(test, feature = \"experimental\"))]"
        )));
    }

    #[test]
    fn attrs_cfg_all_unix_test_is_test() {
        assert!(attrs_imply_test(&parse_attrs("#[cfg(all(unix, test))]")));
    }

    #[test]
    fn attrs_cfg_not_test_is_not_test() {
        // `cfg(not(test))` is production-only code. A naive "any mention of
        // `test`" heuristic would misclassify this — see the regression note
        // in `attr_implies_test`.
        assert!(!attrs_imply_test(&parse_attrs("#[cfg(not(test))]")));
    }

    #[test]
    fn attrs_inline_doc_is_not_test() {
        assert!(!attrs_imply_test(&parse_attrs("/// docs")));
    }

    #[test]
    fn attrs_must_use_is_not_test() {
        assert!(!attrs_imply_test(&parse_attrs("#[must_use]")));
    }

    #[test]
    fn excluded_dir_tests_directory() {
        assert!(is_excluded_dir(Path::new(
            "crates/shipper-core/tests/foo.rs"
        )));
    }

    #[test]
    fn excluded_dir_benches() {
        assert!(is_excluded_dir(Path::new(
            "crates/shipper-core/benches/bench.rs"
        )));
    }

    #[test]
    fn excluded_dir_examples() {
        assert!(is_excluded_dir(Path::new(
            "crates/shipper-core/examples/demo.rs"
        )));
    }

    #[test]
    fn excluded_dir_tests_dot_rs_file() {
        assert!(is_excluded_dir(Path::new(
            "crates/shipper-core/src/state/store/tests.rs"
        )));
    }

    #[test]
    fn excluded_dir_suffix_tests_dot_rs_file() {
        assert!(is_excluded_dir(Path::new(
            "crates/shipper-core/src/ops/process/cross_platform_edge_case_tests.rs"
        )));
    }

    #[test]
    fn excluded_dir_does_not_exclude_production() {
        assert!(!is_excluded_dir(Path::new(
            "crates/shipper-core/src/engine/mod.rs"
        )));
        assert!(!is_excluded_dir(Path::new(
            "crates/shipper-core/src/lib.rs"
        )));
    }

    #[test]
    fn excluded_dir_does_not_exclude_test_helper_in_production_filename() {
        // A file that just happens to have `test` in its name (not as a
        // suffix `_tests.rs`) is still production. The cfg-test attribute
        // walk handles `#[cfg(test)]` blocks inside.
        assert!(!is_excluded_dir(Path::new(
            "crates/shipper-core/src/engine/testable_seam.rs"
        )));
    }
}
