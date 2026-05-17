//! `cargo xtask non-rust propose`
//!
//! Finds tracked non-Rust files that are not yet receipted in
//! `policy/non-rust-allowlist.toml` and writes a draft TOML allowlist
//! plus a human-readable proposal document to `target/policy/`.
//!
//! **This command never mutates `policy/non-rust-allowlist.toml`.** The output
//! is a starting point for a human (or follow-up agent) to review, edit, and
//! intentionally copy into the real ledger. Unknown owner and reason fields
//! are filled with `TODO` so the proposal can never silently land an
//! anonymous receipt.
//!
//! See `docs/policy/NON_RUST_ROLLOUT.md` for the broader rollout doctrine,
//! particularly the rule that `reason = "Scheduled to be converted to
//! Rust/xtask"` is a valid disposition for legacy items.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use globset::Glob;
use serde::Deserialize;

const ALLOWLIST_REL: &str = "policy/non-rust-allowlist.toml";
const OUTPUT_DIR_REL: &str = "target/policy";
const PROPOSED_TOML: &str = "non-rust-proposed-allowlist.toml";
const PROPOSAL_MD: &str = "non-rust-proposal.md";

pub fn propose() -> Result<()> {
    let workspace_root = workspace_root()?;
    let tracked = enumerate_non_rust(&workspace_root)?;
    let entries = load_allowlist(&workspace_root)?;
    let unreceipted = find_unreceipted(&tracked, &entries);

    let today = today_iso();
    let review_after = review_after_iso(&today);

    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let toml_path = out_dir.join(PROPOSED_TOML);
    fs::write(&toml_path, render_toml(&unreceipted, &today, &review_after))
        .with_context(|| format!("writing {}", toml_path.display()))?;

    let md_path = out_dir.join(PROPOSAL_MD);
    fs::write(
        &md_path,
        render_markdown(&unreceipted, &today, &review_after),
    )
    .with_context(|| format!("writing {}", md_path.display()))?;

    println!(
        "wrote {} proposed entr{} to {} (and matching {})",
        unreceipted.len(),
        if unreceipted.len() == 1 { "y" } else { "ies" },
        toml_path.display(),
        PROPOSAL_MD,
    );
    Ok(())
}

// ─── Allowlist deserialization (intentionally duplicated from
// check_file_policy until a follow-up DRY pass) ──────────────────────────────

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
}

enum Selector {
    Path(String),
    Pattern(String),
}

fn load_allowlist(workspace_root: &Path) -> Result<Vec<Selector>> {
    let path = workspace_root.join(ALLOWLIST_REL);
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let doc: AllowlistDoc =
        toml::from_str(&raw).with_context(|| format!("parsing TOML in {}", path.display()))?;

    let mut entries = Vec::with_capacity(doc.file.len() + doc.glob.len());
    for raw in doc.file {
        if let Some(p) = raw.path {
            entries.push(Selector::Path(p));
        }
    }
    for raw in doc.glob {
        if let Some(p) = raw.pattern {
            entries.push(Selector::Pattern(p));
        }
    }
    Ok(entries)
}

fn find_unreceipted(tracked: &[String], entries: &[Selector]) -> Vec<String> {
    tracked
        .iter()
        .filter(|path| !any_match(entries, path))
        .cloned()
        .collect()
}

fn any_match(entries: &[Selector], path: &str) -> bool {
    entries.iter().any(|sel| match sel {
        Selector::Path(p) => p == path,
        Selector::Pattern(pat) => Glob::new(pat)
            .map(|g| g.compile_matcher().is_match(path))
            .unwrap_or(false),
    })
}

// ─── Tracked-file enumeration ───────────────────────────────────────────────

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

fn review_after_iso(today: &str) -> String {
    // 90-day default review window. Honest enough for a first-pass receipt;
    // human review can shorten or extend per entry.
    let parsed = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok();
    parsed
        .and_then(|d| d.checked_add_days(chrono::Days::new(90)))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| today.to_string())
}

// ─── Rendering ──────────────────────────────────────────────────────────────

fn render_toml(unreceipted: &[String], today: &str, review_after: &str) -> String {
    let mut out = String::new();
    out.push_str("# Proposed non-Rust allowlist entries.\n");
    out.push_str("#\n");
    out.push_str("# Generated by `cargo xtask non-rust propose`.\n");
    out.push_str("# Review, edit, and intentionally copy entries into\n");
    out.push_str("# `policy/non-rust-allowlist.toml`. This file is regenerated on every run\n");
    out.push_str("# and is NOT a source of truth. The proposer never edits the real ledger.\n");
    out.push_str("#\n");
    out.push_str("# A valid `reason` may include:\n");
    out.push_str("#   - a durable explanation of why the file exists, or\n");
    out.push_str("#   - \"Scheduled to be converted to Rust/xtask\" for legacy/migration items,\n");
    out.push_str("#     paired with an `expires` date.\n");
    out.push_str("#\n");
    out.push_str("# See docs/policy/NON_RUST_ROLLOUT.md.\n\n");
    out.push_str("schema_version = \"1.0\"\n");
    out.push_str("policy = \"non-rust-allowlist-proposal\"\n");
    out.push_str(&format!("generated_at = \"{}\"\n", today));
    out.push_str("status = \"proposal\"\n\n");

    for path in unreceipted {
        out.push_str("[[file]]\n");
        out.push_str(&format!("path = \"{}\"\n", toml_escape(path)));
        out.push_str("kind = \"TODO\"\n");
        out.push_str("surface = \"TODO\"\n");
        out.push_str("classification = \"TODO\"\n");
        out.push_str("owner = \"TODO\"\n");
        out.push_str(
            "reason = \"TODO: explain why this non-Rust surface remains. If scheduled for conversion to Rust/xtask, say so and add an `expires` date.\"\n",
        );
        out.push_str("covered_by = []\n");
        out.push_str(&format!("created = \"{}\"\n", today));
        out.push_str(&format!("review_after = \"{}\"\n\n", review_after));
    }
    out
}

fn render_markdown(unreceipted: &[String], today: &str, review_after: &str) -> String {
    let mut out = String::new();
    out.push_str("# Non-Rust Allowlist Proposal\n\n");
    out.push_str(&format!(
        "Generated by `cargo xtask non-rust propose` on {}.\n\n",
        today
    ));
    out.push_str("## How to use\n\n");
    out.push_str(&format!(
        "1. Review each draft entry in [`{PROPOSED_TOML}`](./{PROPOSED_TOML}).\n"
    ));
    out.push_str("2. Replace every `TODO` field with a real value. The minimum: a durable `reason`, a real `owner`, and a real `kind`/`surface`/`classification`.\n");
    out.push_str("3. If the entry's true disposition is \"this file will be removed or converted\", set `reason = \"Scheduled to be converted to Rust/xtask\"` and add an `expires` date.\n");
    out.push_str("4. Copy the edited entry into `policy/non-rust-allowlist.toml`.\n");
    out.push_str(
        "5. Re-run `cargo xtask check-file-policy --mode advisory` to verify coverage.\n\n",
    );
    out.push_str("This proposal is regenerated on every run and is not a source of truth.\n\n");

    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Unreceipted non-Rust files: {}\n",
        unreceipted.len()
    ));
    out.push_str(&format!("- Default `created`: `{today}`\n"));
    out.push_str(&format!(
        "- Default `review_after`: `{review_after}` (90 days)\n\n"
    ));

    out.push_str("## Proposed entries grouped by top-level directory\n\n");
    let grouped = group_by_top_dir(unreceipted);
    for (dir, paths) in &grouped {
        out.push_str(&format!("### `{}` ({})\n\n", dir, paths.len()));
        for p in paths {
            out.push_str(&format!("- `{}`\n", p));
        }
        out.push('\n');
    }

    out
}

fn group_by_top_dir(paths: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in paths {
        let top = path
            .split(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(root)".to_string());
        let key = if top == *path {
            "(root)".to_string()
        } else {
            top
        };
        grouped.entry(key).or_default().push(path.clone());
    }
    grouped
}

fn toml_escape(s: &str) -> String {
    // Allowlist paths are tracked-by-git, so they never contain control
    // characters; we only need to escape backslashes and double quotes for
    // a valid TOML basic string.
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
