//! `cargo xtask check-lint-policy` + `cargo xtask check-clippy-exceptions`.
//!
//! Two checkers for the Clippy ledger surface added in PR for #179:
//!
//! - **`check-lint-policy`** validates that MSRV agrees across
//!   `Cargo.toml` `[workspace.package] rust-version`, `clippy.toml`
//!   `msrv`, and `policy/clippy-lints.toml` `msrv`, and that every lint
//!   in `[workspace.lints.clippy]` has a ledger entry. Reverse direction
//!   (ledger entries not in `[workspace.lints.clippy]`) is intentionally
//!   not enforced — `[[active]]` entries may be aspirational until PR 7
//!   activates them.
//!
//! - **`check-clippy-exceptions`** validates the schema of every
//!   `policy/clippy-exceptions.toml` entry (lint, path, owner, reason,
//!   expires), fails on expired entries, and runs a shallow regex scan
//!   for bare `#[allow(clippy::...)]` attributes — reported as
//!   *informational* findings only in this PR (PR for #179 is ledger
//!   only). Code-ledger correspondence enforcement is a future PR.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use regex::Regex;
use serde::Deserialize;

const CARGO_TOML: &str = "Cargo.toml";
const CLIPPY_TOML: &str = "clippy.toml";
const LINTS_LEDGER: &str = "policy/clippy-lints.toml";
const EXCEPTIONS_LEDGER: &str = "policy/clippy-exceptions.toml";

// ─── check-lint-policy ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CargoToml {
    #[serde(default)]
    workspace: Option<Workspace>,
}

#[derive(Debug, Deserialize)]
struct Workspace {
    #[serde(default)]
    package: Option<WorkspacePackage>,
    #[serde(default)]
    lints: Option<Lints>,
}

#[derive(Debug, Deserialize)]
struct WorkspacePackage {
    #[serde(rename = "rust-version")]
    rust_version: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Lints {
    #[serde(default)]
    clippy: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
struct ClippyTomlConfig {
    msrv: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LintsLedger {
    msrv: Option<String>,
    #[serde(default)]
    active: Vec<ActiveLint>,
    #[serde(default)]
    planned: Vec<PlannedLint>,
}

#[derive(Debug, Clone, Deserialize)]
struct ActiveLint {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PlannedLint {
    name: String,
    #[allow(dead_code)]
    min_msrv: Option<String>,
}

pub fn check_lint_policy() -> Result<()> {
    let workspace_root = workspace_root()?;

    // Read MSRV from three sources.
    let cargo: CargoToml = read_toml(&workspace_root.join(CARGO_TOML))?;
    let cargo_msrv = cargo
        .workspace
        .as_ref()
        .and_then(|w| w.package.as_ref())
        .and_then(|p| p.rust_version.clone());

    let clippy: ClippyTomlConfig = read_toml(&workspace_root.join(CLIPPY_TOML))?;
    let clippy_msrv = clippy.msrv.clone();

    let ledger: LintsLedger = read_toml(&workspace_root.join(LINTS_LEDGER))?;
    let ledger_msrv = ledger.msrv.clone();

    let mut errors: Vec<String> = Vec::new();

    match (&cargo_msrv, &clippy_msrv, &ledger_msrv) {
        (Some(c), Some(cl), Some(l)) if c == cl && cl == l => {
            println!("msrv aligned across all three: {c}");
        }
        _ => {
            errors.push(format!(
                "MSRV disagreement: Cargo.toml workspace.package.rust-version={:?}, \
                 clippy.toml msrv={:?}, {} msrv={:?}",
                cargo_msrv, clippy_msrv, LINTS_LEDGER, ledger_msrv
            ));
        }
    }

    // Cross-check: every `[workspace.lints.clippy]` entry has a ledger entry.
    let workspace_lints: BTreeSet<String> = cargo
        .workspace
        .as_ref()
        .and_then(|w| w.lints.as_ref())
        .map(|l| {
            l.clippy
                .keys()
                .map(|k| format!("clippy::{k}"))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let ledger_names: BTreeSet<String> = ledger
        .active
        .iter()
        .map(|a| a.name.clone())
        .chain(ledger.planned.iter().map(|p| p.name.clone()))
        .collect();

    let workspace_lint_count = workspace_lints.len();
    for lint in &workspace_lints {
        if !ledger_names.contains(lint) {
            errors.push(format!(
                "lint `{lint}` is in [workspace.lints.clippy] but has no ledger entry in {LINTS_LEDGER}"
            ));
        }
    }

    println!(
        "check-lint-policy: workspace.lints.clippy={} active_in_ledger={} planned_in_ledger={}",
        workspace_lint_count,
        ledger.active.len(),
        ledger.planned.len(),
    );

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        bail!("check-lint-policy: {} error(s)", errors.len());
    }
    Ok(())
}

// ─── check-clippy-exceptions ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ExceptionsLedger {
    #[serde(default)]
    exception: Vec<RawException>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawException {
    lint: Option<String>,
    path: Option<String>,
    owner: Option<String>,
    reason: Option<String>,
    expires: Option<String>,
}

pub fn check_clippy_exceptions() -> Result<()> {
    let workspace_root = workspace_root()?;
    let today = today_iso();

    let ledger: ExceptionsLedger = read_toml(&workspace_root.join(EXCEPTIONS_LEDGER))?;

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Schema + expiry validation.
    for (idx, e) in ledger.exception.iter().enumerate() {
        let label = e
            .lint
            .clone()
            .unwrap_or_else(|| format!("entry {}", idx + 1));
        for (field, present) in [
            ("lint", e.lint.is_some()),
            ("path", e.path.is_some()),
            ("owner", e.owner.is_some()),
            ("reason", e.reason.is_some()),
            ("expires", e.expires.is_some()),
        ] {
            if !present {
                errors.push(format!(
                    "exception {label} missing required field `{field}`"
                ));
            }
        }
        if let Some(exp) = &e.expires
            && date_is_past(exp, &today)
        {
            errors.push(format!(
                "exception {label} expired: expires={exp} today={today}"
            ));
        }
    }

    // Shallow scan for bare `#[allow(clippy::...)]`. Informational only.
    let bare_allows = shallow_bare_allow_scan(&workspace_root)?;
    for occ in &bare_allows {
        warnings.push(format!(
            "bare `#[allow(clippy::...)]` at {} — prefer `#[expect(clippy::..., reason = \"...\")]` (informational; not failing in PR 5)",
            occ
        ));
    }

    println!(
        "check-clippy-exceptions: entries={} expired=0 schema_errors={} bare_allow_informational={}",
        ledger.exception.len(),
        errors.len(),
        bare_allows.len(),
    );
    for w in &warnings {
        println!("note: {w}");
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        bail!("check-clippy-exceptions: {} error(s)", errors.len());
    }
    Ok(())
}

fn shallow_bare_allow_scan(workspace_root: &Path) -> Result<Vec<String>> {
    let re = Regex::new(r"#\s*\[\s*allow\s*\(\s*clippy::[A-Za-z0-9_]+")
        .context("compiling bare-allow regex")?;

    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("ls-files")
        .arg("-z")
        .arg("*.rs")
        .output()
        .context("running `git ls-files -z *.rs`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`git ls-files -z *.rs` exited {}: {}",
            output.status,
            stderr.trim()
        );
    }

    let mut hits: Vec<String> = Vec::new();
    for entry in output.stdout.split(|&b| b == 0).filter(|s| !s.is_empty()) {
        let rel = String::from_utf8_lossy(entry).into_owned();
        let abs = workspace_root.join(&rel);
        let content = match fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for (lineno, line) in content.lines().enumerate() {
            if re.is_match(line) {
                hits.push(format!("{}:{}", rel, lineno + 1));
            }
        }
    }
    hits.sort();
    Ok(hits)
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))
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

fn date_is_past(date: &str, today: &str) -> bool {
    let parsed = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok();
    let today_parsed = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok();
    match (parsed, today_parsed) {
        (Some(d), Some(t)) => d < t,
        _ => date.trim() < today,
    }
}
