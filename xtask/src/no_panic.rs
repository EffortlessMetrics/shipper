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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use proc_macro2::LineColumn;
use serde::Serialize;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, ExprIndex, ExprMacro, ExprMethodCall, File, ImplItemFn, ItemFn, ItemImpl, ItemMod,
    Meta, TraitItemFn,
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

pub fn baseline() -> Result<()> {
    let workspace_root = workspace_root()?;
    let files = enumerate_source_files(&workspace_root)?;

    let mut all_findings: Vec<Finding> = Vec::new();
    let mut files_with_findings: u64 = 0;
    let total_files = files.len() as u64;

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
    // `**/tests.rs` files and `tests/`/`benches/`/`examples/` directories
    // are already excluded at enumeration time; this drops the remaining
    // test-code findings that hid inside production source files.
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

    let baseline = BaselineFile {
        schema_version: SCHEMA_VERSION,
        generated_by: "cargo xtask no-panic baseline (#187)",
        generated_on: chrono::Utc::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
        note: "Machine-generated. Run `cargo xtask no-panic baseline` to regenerate. \
               See docs/NO_PANIC_POLICY.md for semantics.",
        counts: Counts {
            total_call_sites: total_sites,
            total_entries: entries.len() as u64,
            files_scanned: total_files,
            files_with_findings,
            by_family,
        },
        entries,
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
        dropped_test_sites,
    );
    for (family, n) in &baseline.counts.by_family {
        println!("  {family}: {n}");
    }
    println!("wrote {}", BASELINE_PATH);
    Ok(())
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
        let mut hit = false;
        // `parse_nested_meta` walks the meta items inside `cfg(...)`.
        let _ = attr.parse_nested_meta(|meta| {
            if meta_mentions_test(&meta.path) {
                hit = true;
            }
            // Recurse into `any(test, ...)` / `all(...)` / `not(...)`.
            if meta.path.is_ident("any") || meta.path.is_ident("all") || meta.path.is_ident("not") {
                let _ = meta.parse_nested_meta(|inner| {
                    if meta_mentions_test(&inner.path) {
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
        // Fallback: inspect the meta as a parsed `Meta::List`.
        if let Meta::List(list) = &attr.meta
            && list.tokens.to_string().contains("test")
        {
            return true;
        }
    }
    false
}

fn meta_mentions_test(path: &syn::Path) -> bool {
    path.is_ident("test")
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
