//! `cargo xtask ripr-pr` — thin wrapper around the external `ripr` CLI.
//!
//! ripr (`crates.io/crates/ripr`) is static mutation-exposure analysis
//! authored and maintained by EffortlessMetrics. Shipper *consumes* ripr
//! as an advisory PR lane; this module is intentionally a thin shim. It
//! does NOT implement RIPR analysis — that surface lives in the upstream
//! crate.
//!
//! Local behaviour: if `ripr` is missing on PATH, print install
//! instructions and exit success (advisory). CI installs a pinned version
//! before calling, so the binary is always present there. The wrapper
//! defaults to `ripr pilot --root .` which is the zero-config analysis
//! ripr documents as the first useful invocation.
//!
//! After ripr writes its native outputs under `target/ripr/`, the wrapper
//! projects two of them into `target/policy/ripr-report.{md,json}` so the
//! rest of Shipper's policy tooling (notably `cargo xtask policy-report`)
//! can treat ripr as an eventual ninth policy area without crawling into
//! ripr's per-mode directory layout.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::Value;

const RIPR_INSTALL_HINT: &str =
    "ripr not found on PATH. Install with: `cargo install ripr --locked --version 0.5.0`";

const RIPR_NATIVE_MD: &str = "target/ripr/pilot/pilot-summary.md";
// pilot-summary.json is the compact summary (~13 KB). repo-exposure.json
// and agent-seam-packets.json are also written by `ripr pilot` but each
// runs into tens of MB on real workspaces, which makes them too heavy to
// republish as a policy-report artifact.
const RIPR_NATIVE_JSON: &str = "target/ripr/pilot/pilot-summary.json";
const POLICY_REPORT_MD: &str = "target/policy/ripr-report.md";
const POLICY_REPORT_JSON: &str = "target/policy/ripr-report.json";

/// Arguments for `cargo xtask ripr-pr`. `--base` is forward-looking: `ripr
/// pilot` does not consume it today, but the wrapper accepts it so the CI
/// command line is already shaped for the eventual switch to
/// `ripr check --base <ref>` once that format contract stabilises.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Verify generated PR evidence files instead of invoking ripr.
    #[arg(long)]
    pub check: bool,

    /// PR base ref passed to `ripr check`.
    #[arg(long, default_value = "origin/main")]
    pub base: String,

    /// PR head ref accepted for workflow symmetry. ripr check 0.5 derives head from the working tree.
    #[arg(long, default_value = "HEAD")]
    pub head: String,
}

pub fn ripr_pr(args: &Args) -> Result<()> {
    if args.check {
        check_json_and_markdown(RIPR_PR_JSON, RIPR_PR_MD)?;
        println!("ripr-pr: output contract is intact");
        return Ok(());
    }

    if which_ripr().is_none() {
        // Local advisory: do not fail the developer's session if ripr isn't
        // installed. CI pre-installs a pinned version, so this branch is
        // for local-only invocations.
        println!("{RIPR_INSTALL_HINT}");
        println!("`cargo xtask ripr-pr` exiting advisory-success (no ripr binary).");
        return Ok(());
    }

    fs::create_dir_all(RIPR_PR_DIR).context("creating target/ripr/pr/")?;
    let ripr_bin = ripr_bin();
    let output = Command::new(&ripr_bin)
        .args([
            "check",
            "--root",
            ".",
            "--base",
            &args.base,
            "--format",
            "repo-exposure-json",
        ])
        .current_dir(workspace_root_path())
        .output()
        .with_context(|| format!("spawning `{ripr_bin} check`"))?;

    if !output.status.success() {
        eprintln!(
            "ripr check exited with status {} — findings are advisory; see target/ripr/",
            output.status.code().unwrap_or(-1)
        );
        eprintln!("{}", String::from_utf8_lossy(&output.stderr).trim());
    } else {
        fs::write(RIPR_PR_JSON, &output.stdout)
            .with_context(|| format!("writing {RIPR_PR_JSON}"))?;
        let value: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("parsing {RIPR_PR_JSON}"))?;
        write_pr_markdown_from_json(&value)?;
    }

    project_to_policy_report().context("projecting ripr outputs to target/policy/ripr-report.*")?;
    Ok(())
}

/// Copy ripr's native pilot outputs into `target/policy/ripr-report.{md,json}`
/// so they sit alongside the other policy reports. Each side is best-effort:
/// if ripr did not produce a given output (e.g. analysis failed before
/// writing), skip silently rather than fail the wrapper.
fn project_to_policy_report() -> Result<()> {
    let dst_dir = Path::new("target/policy");
    fs::create_dir_all(dst_dir).context("creating target/policy/")?;

    project_one(RIPR_NATIVE_MD, POLICY_REPORT_MD)?;
    project_one(RIPR_NATIVE_JSON, POLICY_REPORT_JSON)?;
    Ok(())
}

fn project_one(src: &str, dst: &str) -> Result<()> {
    let src_path = Path::new(src);
    if !src_path.exists() {
        // Quiet skip — ripr may not have written this output (e.g. it
        // bailed early). The CI workflow uploads target/ripr/ either way.
        return Ok(());
    }
    fs::copy(src_path, dst).with_context(|| format!("copying {src} -> {dst}"))?;
    Ok(())
}

fn which_ripr() -> Option<()> {
    // Cross-platform "is ripr on PATH?" using `--version` as a lightweight
    // probe. Avoid `which`/`where` to keep the dependency surface flat.
    let status = Command::new(ripr_bin())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Some(()),
        Ok(_) | Err(_) => None,
    }
}

// ─── generated badge endpoints ────────────────────────────────────────────
//
// Public README badges are repo-scoped trust markers. They must never be
// generated from PR/diff-scoped evidence. The committed endpoint files under
// badges/ are intentionally tiny Shields projections; detailed ripr reports
// stay under target/.

const BADGE_ENDPOINT_DIR: &str = "badges";
const BADGE_ENDPOINT_TARGET_DIR: &str = "target/xtask/badges";
const RIPR_PLUS_BADGE: &str = "ripr-plus.json";
const RIPR_REPO_BADGE_REPORT: &str = "target/ripr/check-repo.json";

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct ShieldsEndpointBadge {
    #[serde(rename = "schemaVersion")]
    schema_version: u8,
    label: String,
    message: String,
    color: String,
}

#[derive(Debug, clap::Args)]
pub struct BadgeArgs {
    /// Check committed badge endpoints for drift without updating badges/.
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, clap::Args)]
pub struct ReviewCommentsArgs {
    /// Verify generated review-comment files instead of invoking ripr.
    #[arg(long)]
    pub check: bool,

    /// PR base ref passed to `ripr review-comments`.
    #[arg(long, default_value = "origin/main")]
    pub base: String,

    /// PR head ref passed to `ripr review-comments`.
    #[arg(long, default_value = "HEAD")]
    pub head: String,
}

pub fn badges(args: &BadgeArgs) -> Result<()> {
    let workspace_root = workspace_root_path();
    let target_dir = workspace_root.join(BADGE_ENDPOINT_TARGET_DIR);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("creating {}", target_dir.display()))?;

    let ripr_plus = ripr_plus_badge(&workspace_root)?;
    validate_shields_badge(&ripr_plus, Some("ripr+"))?;
    write_json_pretty(&target_dir.join(RIPR_PLUS_BADGE), &ripr_plus)?;

    let committed_dir = workspace_root.join(BADGE_ENDPOINT_DIR);
    if args.check {
        compare_files(
            &committed_dir.join(RIPR_PLUS_BADGE),
            &target_dir.join(RIPR_PLUS_BADGE),
        )?;
        println!("badges: committed endpoints are current");
        return Ok(());
    }

    fs::create_dir_all(&committed_dir)
        .with_context(|| format!("creating {}", committed_dir.display()))?;
    fs::copy(
        target_dir.join(RIPR_PLUS_BADGE),
        committed_dir.join(RIPR_PLUS_BADGE),
    )
    .with_context(|| format!("copying {RIPR_PLUS_BADGE} into badges/"))?;

    println!("badges: refreshed public endpoint JSON under badges/");
    Ok(())
}

pub fn repo_badge_artifacts() -> Result<()> {
    badges(&BadgeArgs { check: false })
}

fn workspace_root_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under the workspace root")
        .to_path_buf()
}

fn ripr_bin() -> String {
    std::env::var("RIPR_BIN").unwrap_or_else(|_| "ripr".to_string())
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
        .with_context(|| format!("spawning `{ripr_bin} check`"))?;

    if output.status.success() {
        persist_ripr_badge_report(workspace_root, &output.stdout)?;
        return serde_json::from_slice(&output.stdout)
            .with_context(|| format!("{ripr_bin} emitted invalid Shields endpoint JSON"));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("test-efficiency.json") {
        bail!(
            "{ripr_bin} repo-badge-plus-shields failed with status {}: {}",
            output.status,
            stderr.trim()
        );
    }

    eprintln!(
        "warning: {ripr_bin} repo-badge-plus-shields requires test-efficiency.json; falling back to repo-scoped exposure count"
    );
    fallback_repo_exposure_badge(workspace_root, &ripr_bin)
}

fn fallback_repo_exposure_badge(
    workspace_root: &Path,
    ripr_bin: &str,
) -> Result<ShieldsEndpointBadge> {
    let output = Command::new(ripr_bin)
        .arg("check")
        .arg("--root")
        .arg(workspace_root)
        .arg("--mode")
        .arg("ready")
        .arg("--format")
        .arg("repo-exposure-json")
        .current_dir(workspace_root)
        .output()
        .with_context(|| format!("spawning `{ripr_bin} check --format repo-exposure-json`"))?;

    if !output.status.success() {
        bail!(
            "{ripr_bin} repo-exposure-json fallback failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    persist_ripr_badge_report(workspace_root, &output.stdout)?;
    let value: Value = serde_json::from_slice(&output.stdout)
        .context("parsing ripr repo-exposure JSON for badge fallback")?;
    let headline = value
        .get("metrics")
        .and_then(|m| m.get("headline_eligible"))
        .and_then(|v| v.as_u64())
        .context("`metrics.headline_eligible` missing from ripr repo-exposure JSON")?;

    Ok(ShieldsEndpointBadge {
        schema_version: 1,
        label: "ripr+".to_string(),
        message: headline.to_string(),
        color: shields_color(headline).to_string(),
    })
}

fn persist_ripr_badge_report(workspace_root: &Path, bytes: &[u8]) -> Result<()> {
    let report_path = workspace_root.join(RIPR_REPO_BADGE_REPORT);
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&report_path, bytes).with_context(|| format!("writing {}", report_path.display()))
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

fn write_json_pretty(path: &Path, badge: &ShieldsEndpointBadge) -> Result<()> {
    let mut json = serde_json::to_string_pretty(badge)
        .with_context(|| format!("serialising {}", path.display()))?;
    json.push('\n');
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

fn compare_files(committed: &Path, generated: &Path) -> Result<()> {
    let committed_bytes = fs::read(committed)
        .with_context(|| format!("reading committed badge {}", committed.display()))?;
    let generated_bytes = fs::read(generated)
        .with_context(|| format!("reading generated badge {}", generated.display()))?;
    if committed_bytes != generated_bytes {
        bail!(
            "badge endpoint drift: {} differs from {}; run `cargo xtask badges`",
            committed.display(),
            generated.display()
        );
    }
    Ok(())
}

// ─── RIPR PR review guidance ────────────────────────────────────────────────

const RIPR_PR_DIR: &str = "target/ripr/pr";
const RIPR_PR_JSON: &str = "target/ripr/pr/repo-exposure.json";
const RIPR_PR_MD: &str = "target/ripr/pr/repo-exposure.md";
const RIPR_REVIEW_DIR: &str = "target/ripr/review";
const RIPR_REVIEW_JSON: &str = "target/ripr/review/comments.json";
const RIPR_REVIEW_MD: &str = "target/ripr/review/comments.md";

pub fn ripr_review_comments(args: &ReviewCommentsArgs) -> Result<()> {
    if args.check {
        check_json_and_markdown(RIPR_REVIEW_JSON, RIPR_REVIEW_MD)?;
        println!("ripr-review-comments: output contract is intact");
        return Ok(());
    }

    if which_ripr().is_none() {
        println!("{RIPR_INSTALL_HINT}");
        println!("`cargo xtask ripr-review-comments` exiting advisory-success (no ripr binary).");
        return Ok(());
    }

    fs::create_dir_all(RIPR_REVIEW_DIR).context("creating target/ripr/review/")?;
    let ripr_bin = ripr_bin();
    let status = Command::new(&ripr_bin)
        .args([
            "review-comments",
            "--root",
            ".",
            "--base",
            &args.base,
            "--head",
            &args.head,
            "--out",
            RIPR_REVIEW_JSON,
        ])
        .current_dir(workspace_root_path())
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

fn shields_color(count: u64) -> &'static str {
    match count {
        0 => "brightgreen",
        1..=99 => "yellowgreen",
        100..=999 => "orange",
        _ => "red",
    }
}

fn check_json_and_markdown(json_path: &str, md_path: &str) -> Result<()> {
    let json = fs::read_to_string(json_path).with_context(|| format!("reading {json_path}"))?;
    let _: Value = serde_json::from_str(&json).with_context(|| format!("parsing {json_path}"))?;
    let md = fs::read_to_string(md_path).with_context(|| format!("reading {md_path}"))?;
    if md.trim().is_empty() {
        bail!("{md_path} is empty");
    }
    Ok(())
}

fn write_pr_markdown_from_json(json: &Value) -> Result<()> {
    let metrics = json.get("metrics").unwrap_or(json);
    let mut md = String::from("# RIPR PR Exposure\n\n");
    md.push_str("PR-scoped static exposure evidence generated by `cargo xtask ripr-pr`.\n\n");
    if let Some(obj) = metrics.as_object() {
        md.push_str("## Metrics\n\n");
        for key in [
            "findings",
            "exposed",
            "weakly_exposed",
            "reachable_unrevealed",
            "no_static_path",
            "headline_eligible",
        ] {
            if let Some(value) = obj.get(key) {
                md.push_str(&format!("- `{key}`: `{value}`\n"));
            }
        }
    }
    fs::write(RIPR_PR_MD, md).with_context(|| format!("writing {RIPR_PR_MD}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hint_mentions_pinned_version() {
        // Guard against a future bump to the install command that forgets
        // to update the user-facing hint.
        assert!(RIPR_INSTALL_HINT.contains("cargo install ripr"));
        assert!(RIPR_INSTALL_HINT.contains("--locked"));
        assert!(RIPR_INSTALL_HINT.contains("--version"));
    }

    #[test]
    fn install_hint_pinned_version_matches_workflow() {
        // The pinned version in the hint and in .github/workflows/ripr.yml
        // must stay in sync. The workflow file is read at test time so any
        // bump in one place flags the other. The hint wraps the command in
        // backticks, so trim non-version chars off the parsed tail.
        let workflow = include_str!("../../.github/workflows/ripr.yml");
        let tail = RIPR_INSTALL_HINT
            .rsplit_once("--version ")
            .map(|(_, v)| v.trim())
            .expect("install hint includes `--version <X>`");
        let pin = tail.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
        assert!(
            workflow.contains(&format!("--version {pin}")),
            ".github/workflows/ripr.yml does not pin ripr at version {pin} \
             (xtask install hint and workflow are out of sync)"
        );
    }

    #[test]
    fn project_one_skips_missing_source() {
        // Best-effort copy: if ripr did not produce a given output, the
        // wrapper should not fail. Use a clearly-non-existent path so this
        // is deterministic across hosts.
        let result = project_one(
            "target/this/path/does/not/exist.txt",
            "target/policy/ripr-projection-skip-probe.txt",
        );
        assert!(result.is_ok());
        assert!(
            !Path::new("target/policy/ripr-projection-skip-probe.txt").exists(),
            "no destination should be written when the source is missing"
        );
    }

    #[test]
    fn shields_color_thresholds() {
        // Guard against accidental threshold drift — these influence the
        // visual signal of the public badges.
        assert_eq!(shields_color(0), "brightgreen");
        assert_eq!(shields_color(1), "yellowgreen");
        assert_eq!(shields_color(99), "yellowgreen");
        assert_eq!(shields_color(100), "orange");
        assert_eq!(shields_color(999), "orange");
        assert_eq!(shields_color(1000), "red");
        assert_eq!(shields_color(u64::MAX), "red");
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
    fn write_json_pretty_uses_shields_keys() {
        let dir = std::env::temp_dir().join("shipper-ripr-badge-test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("ripr-plus-probe.json");
        let badge = ShieldsEndpointBadge {
            schema_version: 1,
            label: "ripr+".to_string(),
            message: "42".to_string(),
            color: "yellowgreen".to_string(),
        };
        write_json_pretty(&path, &badge).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["label"], "ripr+");
        assert_eq!(v["message"], "42");
        assert_eq!(v["color"], "yellowgreen");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn args_default_base_is_origin_main() {
        // Clap defaults can drift quietly; pin the expected default so a
        // future refactor that changes the base default flags it loudly.
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
