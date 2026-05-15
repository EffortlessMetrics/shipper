//! `cargo xtask ripr-*` — thin wrappers around the external `ripr` CLI.
//!
//! ripr (`crates.io/crates/ripr`) is static mutation-exposure analysis
//! authored and maintained by EffortlessMetrics. Shipper *consumes* ripr
//! as advisory PR evidence and as a repo-scoped public badge input; this
//! module is intentionally a thin shim. It does NOT implement RIPR analysis.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const RIPR_INSTALL_HINT: &str =
    "ripr not found on PATH. Install with: `cargo install ripr --locked --version 0.5.0`";

const RIPR_PR_JSON: &str = "target/ripr/pr/repo-exposure.json";
const RIPR_PR_MD: &str = "target/ripr/pr/repo-exposure.md";
const RIPR_REVIEW_JSON: &str = "target/ripr/review/comments.json";
const RIPR_REVIEW_MD: &str = "target/ripr/review/comments.md";
const POLICY_REPORT_MD: &str = "target/policy/ripr-report.md";
const POLICY_REPORT_JSON: &str = "target/policy/ripr-report.json";

const BADGE_ENDPOINT_DIR: &str = "badges";
const BADGE_ENDPOINT_TARGET_DIR: &str = "target/xtask/badges";

/// Arguments for `cargo xtask ripr-pr`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// PR base ref.
    #[arg(long, default_value = "origin/main")]
    pub base: String,

    /// PR head ref. Reserved for contract symmetry; `ripr check` derives the head from the working tree.
    #[arg(long, default_value = "HEAD")]
    pub head: String,

    /// Verify required PR evidence files already exist and are readable.
    #[arg(long)]
    pub check: bool,
}

/// Arguments for `cargo xtask ripr-review-comments`.
#[derive(Debug, clap::Args)]
pub struct ReviewArgs {
    /// PR base ref.
    #[arg(long, default_value = "origin/main")]
    pub base: String,

    /// PR head ref.
    #[arg(long, default_value = "HEAD")]
    pub head: String,

    /// Verify required review-guidance files already exist and are readable.
    #[arg(long)]
    pub check: bool,
}

/// Arguments for `cargo xtask badges`.
#[derive(Debug, clap::Args)]
pub struct BadgeArgs {
    /// Regenerate into target/xtask/badges and fail if committed endpoints drift.
    #[arg(long)]
    pub check: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct ShieldsEndpointBadge {
    #[serde(rename = "schemaVersion")]
    schema_version: u8,
    label: String,
    message: String,
    color: String,
}

pub fn ripr_pr(args: &Args) -> Result<()> {
    if args.check {
        return check_ripr_pr_outputs(&workspace_root_path());
    }

    if which_ripr().is_none() {
        // Local advisory: do not fail the developer's session if ripr isn't
        // installed. CI pre-installs ripr, so this branch is for local-only
        // invocations.
        println!("{RIPR_INSTALL_HINT}");
        println!("`cargo xtask ripr-pr` exiting advisory-success (no ripr binary).");
        return Ok(());
    }

    let workspace_root = workspace_root_path();
    let ripr_bin = ripr_bin();
    let out_path = workspace_root.join(RIPR_PR_JSON);
    ensure_parent(&out_path)?;

    let output = Command::new(&ripr_bin)
        .arg("check")
        .arg("--root")
        .arg(&workspace_root)
        .arg("--base")
        .arg(&args.base)
        .arg("--format")
        .arg("repo-exposure-json")
        .current_dir(&workspace_root)
        .output()
        .with_context(|| format!("spawning `{ripr_bin} check` for PR evidence"))?;

    if !output.status.success() {
        // ripr findings are advisory by policy — surface its exit code as an
        // annotation but do not propagate non-zero out for the producer path.
        eprintln!(
            "ripr PR evidence exited with status {} — findings are advisory; see target/ripr/",
            output.status.code().unwrap_or(-1)
        );
        if !output.stderr.is_empty() {
            eprintln!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
    }

    if !output.stdout.is_empty() {
        fs::write(&out_path, &output.stdout)
            .with_context(|| format!("writing {}", out_path.display()))?;
        let value: Value =
            serde_json::from_slice(&output.stdout).context("parsing ripr PR repo-exposure JSON")?;
        write_repo_exposure_markdown(&workspace_root.join(RIPR_PR_MD), &value)?;
        project_pr_to_policy_report(&workspace_root)?;
    }

    Ok(())
}

pub fn ripr_review_comments(args: &ReviewArgs) -> Result<()> {
    let workspace_root = workspace_root_path();
    if args.check {
        return check_ripr_review_outputs(&workspace_root);
    }

    if which_ripr().is_none() {
        println!("{RIPR_INSTALL_HINT}");
        println!("`cargo xtask ripr-review-comments` exiting advisory-success (no ripr binary).");
        return Ok(());
    }

    let ripr_bin = ripr_bin();
    let out_path = workspace_root.join(RIPR_REVIEW_JSON);
    ensure_parent(&out_path)?;

    let status = Command::new(&ripr_bin)
        .arg("review-comments")
        .arg("--root")
        .arg(&workspace_root)
        .arg("--base")
        .arg(&args.base)
        .arg("--head")
        .arg(&args.head)
        .arg("--out")
        .arg(&out_path)
        .current_dir(&workspace_root)
        .status()
        .with_context(|| format!("spawning `{ripr_bin} review-comments`"))?;

    if !status.success() {
        eprintln!(
            "ripr review-comments exited with status {} — guidance is advisory; see target/ripr/review/",
            status.code().unwrap_or(-1)
        );
    }

    Ok(())
}

pub fn badges(args: &BadgeArgs) -> Result<()> {
    badges_impl(args.check)
}

/// Back-compat alias for the previous command name used by policy receipts.
pub fn repo_badge_artifacts() -> Result<()> {
    badges_impl(false)
}

fn badges_impl(check: bool) -> Result<()> {
    let workspace_root = workspace_root_path();
    let target_dir = workspace_root.join(BADGE_ENDPOINT_TARGET_DIR);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("creating {}", target_dir.display()))?;

    ensure_test_efficiency_report(&workspace_root)?;

    let ripr_plus = ripr_plus_badge(&workspace_root)?;
    validate_shields_badge(&ripr_plus, Some("ripr+"))?;
    write_json_pretty(&target_dir.join("ripr-plus.json"), &ripr_plus)?;

    if check {
        compare_files(
            &workspace_root
                .join(BADGE_ENDPOINT_DIR)
                .join("ripr-plus.json"),
            &target_dir.join("ripr-plus.json"),
        )?;
        println!("badges: committed endpoints are current");
        return Ok(());
    }

    let committed_dir = workspace_root.join(BADGE_ENDPOINT_DIR);
    fs::create_dir_all(&committed_dir)
        .with_context(|| format!("creating {}", committed_dir.display()))?;
    fs::copy(
        target_dir.join("ripr-plus.json"),
        committed_dir.join("ripr-plus.json"),
    )
    .with_context(|| "copying ripr-plus endpoint into badges/".to_string())?;

    println!("badges: refreshed public endpoint JSON under badges/");
    Ok(())
}

fn ensure_test_efficiency_report(workspace_root: &Path) -> Result<()> {
    let path = workspace_root.join("target/ripr/reports/test-efficiency.json");
    if path.exists() {
        return Ok(());
    }

    ensure_parent(&path)?;
    let report = serde_json::json!({
        "schema_version": "0.1",
        "tests": [],
        "metrics": {
            "tests_scanned": 0,
            "reason_counts": {}
        }
    });
    let mut raw = serde_json::to_string_pretty(&report)
        .context("serialising placeholder test-efficiency report")?;
    raw.push('\n');
    fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn ripr_plus_badge(workspace_root: &Path) -> Result<ShieldsEndpointBadge> {
    let ripr_bin = ripr_bin();
    let output = Command::new(&ripr_bin)
        .arg("check")
        .arg("--root")
        .arg(workspace_root)
        .arg("--format")
        .arg("repo-badge-plus-shields")
        .current_dir(workspace_root)
        .output()
        .with_context(|| format!("spawning `{ripr_bin} check` for repo badge"))?;

    if !output.status.success() {
        bail!(
            "{ripr_bin} repo-badge-plus-shields failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{ripr_bin} emitted invalid Shields endpoint JSON"))
}

fn validate_shields_badge(
    badge: &ShieldsEndpointBadge,
    expected_label: Option<&str>,
) -> Result<()> {
    if badge.schema_version != 1 {
        bail!("badge `{}` has unsupported schemaVersion", badge.label);
    }

    if let Some(expected_label) = expected_label
        && badge.label != expected_label
    {
        bail!(
            "badge label drifted: got `{}`, expected `{expected_label}`",
            badge.label
        );
    }

    if badge.message.trim().is_empty() {
        bail!("badge `{}` has empty message", badge.label);
    }

    if badge.color.trim().is_empty() {
        bail!("badge `{}` has empty color", badge.label);
    }

    Ok(())
}

fn check_ripr_pr_outputs(workspace_root: &Path) -> Result<()> {
    let json_path = workspace_root.join(RIPR_PR_JSON);
    let md_path = workspace_root.join(RIPR_PR_MD);
    require_nonempty_file(&json_path)?;
    require_nonempty_file(&md_path)?;
    let raw = fs::read(&json_path).with_context(|| format!("reading {}", json_path.display()))?;
    let _: Value = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing {} as JSON", json_path.display()))?;
    println!("ripr-pr: output contract is intact");
    Ok(())
}

fn check_ripr_review_outputs(workspace_root: &Path) -> Result<()> {
    let json_path = workspace_root.join(RIPR_REVIEW_JSON);
    let md_path = workspace_root.join(RIPR_REVIEW_MD);
    require_nonempty_file(&json_path)?;
    require_nonempty_file(&md_path)?;
    let raw = fs::read(&json_path).with_context(|| format!("reading {}", json_path.display()))?;
    let _: Value = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing {} as JSON", json_path.display()))?;
    println!("ripr-review-comments: output contract is intact");
    Ok(())
}

fn write_repo_exposure_markdown(path: &Path, value: &Value) -> Result<()> {
    ensure_parent(path)?;
    let metrics = value.get("metrics").and_then(Value::as_object);
    let headline = metrics
        .and_then(|m| m.get("headline_eligible"))
        .and_then(Value::as_u64)
        .map_or_else(|| "unknown".to_string(), |v| v.to_string());
    let exposed = metrics
        .and_then(|m| m.get("exposed"))
        .and_then(Value::as_u64)
        .map_or_else(|| "unknown".to_string(), |v| v.to_string());
    let weakly_exposed = metrics
        .and_then(|m| m.get("weakly_exposed"))
        .and_then(Value::as_u64)
        .map_or_else(|| "unknown".to_string(), |v| v.to_string());

    let markdown = format!(
        "# RIPR PR Evidence\n\n\
         PR-scoped static exposure evidence generated from `ripr check`.\n\n\
         | Metric | Value |\n\
         | --- | --- |\n\
         | Headline eligible | `{headline}` |\n\
         | Exposed | `{exposed}` |\n\
         | Weakly exposed | `{weakly_exposed}` |\n\n\
         JSON evidence: `{RIPR_PR_JSON}`\n"
    );
    fs::write(path, markdown).with_context(|| format!("writing {}", path.display()))
}

fn project_pr_to_policy_report(workspace_root: &Path) -> Result<()> {
    let policy_dir = workspace_root.join("target/policy");
    fs::create_dir_all(&policy_dir)
        .with_context(|| format!("creating {}", policy_dir.display()))?;
    project_one(
        &workspace_root.join(RIPR_PR_MD),
        &workspace_root.join(POLICY_REPORT_MD),
    )?;
    project_one(
        &workspace_root.join(RIPR_PR_JSON),
        &workspace_root.join(POLICY_REPORT_JSON),
    )?;
    Ok(())
}

fn project_one(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    fs::copy(src, dst)
        .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

fn write_json_pretty(path: &Path, badge: &ShieldsEndpointBadge) -> Result<()> {
    ensure_parent(path)?;
    let mut raw = serde_json::to_string_pretty(badge)
        .with_context(|| format!("serialising {}", path.display()))?;
    raw.push('\n');
    fs::write(path, raw).with_context(|| format!("writing {}", path.display()))
}

fn compare_files(committed: &Path, generated: &Path) -> Result<()> {
    let committed_raw = fs::read(committed)
        .with_context(|| format!("reading committed badge endpoint {}", committed.display()))?;
    let generated_raw = fs::read(generated)
        .with_context(|| format!("reading generated badge endpoint {}", generated.display()))?;
    if committed_raw != generated_raw {
        bail!(
            "badge endpoint drift: {} differs from {}; run `cargo xtask badges`",
            committed.display(),
            generated.display()
        );
    }
    Ok(())
}

fn require_nonempty_file(path: &Path) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("missing required file {}", path.display()))?;
    if !metadata.is_file() || metadata.len() == 0 {
        bail!(
            "required file {} is empty or not a regular file",
            path.display()
        );
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(())
}

fn which_ripr() -> Option<()> {
    let output = Command::new(ripr_bin()).arg("--version").output();
    match output {
        Ok(o) if o.status.success() => Some(()),
        Ok(_) | Err(_) => None,
    }
}

fn ripr_bin() -> String {
    std::env::var("RIPR_BIN").unwrap_or_else(|_| "ripr".to_string())
}

fn workspace_root_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under the workspace root")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hint_mentions_pinned_version() {
        assert!(RIPR_INSTALL_HINT.contains("cargo install ripr"));
        assert!(RIPR_INSTALL_HINT.contains("--locked"));
        assert!(RIPR_INSTALL_HINT.contains("--version"));
    }

    #[test]
    fn project_one_skips_missing_source() {
        let result = project_one(
            Path::new("target/this/path/does/not/exist.txt"),
            Path::new("target/policy/ripr-projection-skip-probe.txt"),
        );
        assert!(result.is_ok());
        assert!(
            !Path::new("target/policy/ripr-projection-skip-probe.txt").exists(),
            "no destination should be written when the source is missing"
        );
    }

    #[test]
    fn ripr_plus_badge_shape_is_stable() {
        let badge = ShieldsEndpointBadge {
            schema_version: 1,
            label: "ripr+".to_string(),
            message: "0".to_string(),
            color: "brightgreen".to_string(),
        };

        validate_shields_badge(&badge, Some("ripr+")).unwrap();
    }

    #[test]
    fn scanner_safe_badge_shape_is_stable() {
        let badge = ShieldsEndpointBadge {
            schema_version: 1,
            label: "fixtures".to_string(),
            message: "scanner-safe".to_string(),
            color: "brightgreen".to_string(),
        };

        validate_shields_badge(&badge, Some("fixtures")).unwrap();
    }

    #[test]
    fn validate_shields_badge_rejects_empty_message() {
        let badge = ShieldsEndpointBadge {
            schema_version: 1,
            label: "ripr+".to_string(),
            message: " ".to_string(),
            color: "brightgreen".to_string(),
        };

        assert!(validate_shields_badge(&badge, Some("ripr+")).is_err());
    }

    #[test]
    fn args_default_base_is_origin_main() {
        use clap::Parser;
        #[derive(Parser, Debug)]
        struct Probe {
            #[command(flatten)]
            args: Args,
        }
        let parsed = Probe::parse_from(["probe"]);
        assert_eq!(parsed.args.base, "origin/main");
        assert_eq!(parsed.args.head, "HEAD");
        assert!(!parsed.args.check);
    }
}
