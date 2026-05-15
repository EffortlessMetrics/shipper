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
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
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
    /// PR base ref. Currently advisory only — `ripr pilot` operates on
    /// the working tree, not a diff. Kept on the CLI surface so the
    /// invocation shape ("`cargo xtask ripr-pr --base origin/main`")
    /// stays stable across future wrapper revisions.
    #[arg(long, default_value = "origin/main")]
    pub base: String,

    /// Validate the PR evidence output contract instead of running ripr.
    #[arg(long)]
    pub check: bool,
}

pub fn ripr_pr(args: &Args) -> Result<()> {
    if args.check {
        return check_pr_contract(&workspace_root()?);
    }

    if which_ripr().is_none() {
        // Local advisory: do not fail the developer's session if ripr isn't
        // installed. CI pre-installs a pinned version, so this branch is
        // for local-only invocations.
        println!("{RIPR_INSTALL_HINT}");
        println!("`cargo xtask ripr-pr` exiting advisory-success (no ripr binary).");
        return Ok(());
    }

    // `ripr pilot` is the zero-config analysis. `args.base` is not passed
    // through today (pilot has no `--base` flag); it is reserved for the
    // forthcoming `ripr check --base <ref>` invocation. Acknowledge the
    // value so a stale CI argument does not look like a silent drop.
    if args.base != "origin/main" {
        eprintln!(
            "note: ripr pilot does not consume --base today; received `{}` (ignored)",
            args.base
        );
    }

    let ripr_bin = ripr_bin();
    let status = Command::new(&ripr_bin)
        .args(["pilot", "--root", "."])
        .status()
        .with_context(|| format!("spawning `{ripr_bin} pilot --root .`"))?;

    if !status.success() {
        // ripr findings are advisory by policy — surface its exit code as
        // an `eprintln!` annotation but do not propagate non-zero out.
        // CI's `continue-on-error: true` belt-and-braces this anyway, but
        // local invocations of `cargo xtask ripr-pr` should also be
        // advisory.
        eprintln!(
            "ripr pilot exited with status {} — findings are advisory; see target/ripr/",
            status.code().unwrap_or(-1)
        );
    }

    project_to_policy_report().context("projecting ripr outputs to target/policy/ripr-report.*")?;
    project_to_pr_contract().context("projecting ripr outputs to target/ripr/pr/*")?;
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

const PR_EVIDENCE_DIR: &str = "target/ripr/pr";
const PR_REPO_EXPOSURE_JSON: &str = "target/ripr/pr/repo-exposure.json";
const PR_REPO_EXPOSURE_MD: &str = "target/ripr/pr/repo-exposure.md";
const PILOT_REPO_EXPOSURE_JSON: &str = "target/ripr/pilot/repo-exposure.json";
const PILOT_REPO_EXPOSURE_MD: &str = "target/ripr/pilot/repo-exposure.md";

fn project_to_pr_contract() -> Result<()> {
    fs::create_dir_all(PR_EVIDENCE_DIR).context("creating target/ripr/pr/")?;
    remove_stale_projection(PR_REPO_EXPOSURE_JSON)?;
    remove_stale_projection(PR_REPO_EXPOSURE_MD)?;
    project_first_existing(
        &[PILOT_REPO_EXPOSURE_JSON, RIPR_NATIVE_JSON],
        PR_REPO_EXPOSURE_JSON,
    )?;
    project_first_existing(
        &[PILOT_REPO_EXPOSURE_MD, RIPR_NATIVE_MD],
        PR_REPO_EXPOSURE_MD,
    )?;
    Ok(())
}

fn remove_stale_projection(path: &str) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("removing stale {path}")),
    }
}

fn project_first_existing(sources: &[&str], dst: &str) -> Result<()> {
    if let Some(src) = sources.iter().find(|src| Path::new(src).exists()) {
        return project_one(src, dst);
    }
    Ok(())
}

fn check_pr_contract(workspace_root: &Path) -> Result<()> {
    validate_json_file(&workspace_root.join(PR_REPO_EXPOSURE_JSON))?;
    validate_nonempty_file(&workspace_root.join(PR_REPO_EXPOSURE_MD))?;
    println!("ripr-pr: required PR evidence files are present");
    Ok(())
}

fn which_ripr() -> Option<()> {
    // Cross-platform "is ripr on PATH?" using `--version` as a lightweight
    // probe. Avoid `which`/`where` to keep the dependency surface flat.
    let output = Command::new(ripr_bin()).arg("--version").output();
    match output {
        Ok(o) if o.status.success() => Some(()),
        Ok(_) | Err(_) => None,
    }
}

// ─── badges ────────────────────────────────────────────────────────────────
//
// Repo-scoped Shields endpoint JSON for public README badges. Per ripr's badge
// policy, README badges must be repo-scoped: PR/diff-scoped artifacts belong in
// CI summaries and uploads, not in public trust markers.

const BADGE_ENDPOINT_DIR: &str = "badges";
const BADGE_ENDPOINT_TARGET_DIR: &str = "target/xtask/badges";
const SHIELDS_RIPR_PLUS_PATH: &str = "badges/ripr-plus.json";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct ShieldsEndpointBadge {
    #[serde(rename = "schemaVersion")]
    schema_version: u8,
    label: String,
    message: String,
    color: String,
}

#[derive(Debug, clap::Args)]
pub struct BadgesArgs {
    /// Check committed endpoint JSON for drift without updating badges/.
    #[arg(long)]
    pub check: bool,
}

pub fn badges(args: &BadgesArgs) -> Result<()> {
    let workspace_root = workspace_root()?;
    let target_dir = workspace_root.join(BADGE_ENDPOINT_TARGET_DIR);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("creating {}", target_dir.display()))?;

    let ripr_plus = ripr_plus_badge(&workspace_root)?;
    validate_shields_badge(&ripr_plus, Some("ripr+"))?;
    write_json_pretty(&target_dir.join("ripr-plus.json"), &ripr_plus)?;

    if args.check {
        compare_files(
            &workspace_root.join(SHIELDS_RIPR_PLUS_PATH),
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
    .context("copying generated ripr-plus badge into badges/")?;

    println!("badges: refreshed public endpoint JSON under badges/");
    Ok(())
}

pub fn repo_badge_artifacts() -> Result<()> {
    badges(&BadgesArgs { check: false })
}

fn ripr_plus_badge(workspace_root: &Path) -> Result<ShieldsEndpointBadge> {
    let ripr_bin = ripr_bin();

    match run_ripr_badge_format(&ripr_bin, workspace_root, "repo-badge-plus-shields") {
        Ok(badge) => return Ok(badge),
        Err(err) if can_fallback_to_repo_badge(&err.to_string()) => {
            eprintln!(
                "note: {ripr_bin} repo-badge-plus-shields was unavailable ({err}); \
                 falling back to repo-badge-shields and preserving the public ripr+ label"
            );
        }
        Err(err) => return Err(err),
    }

    let mut badge = run_ripr_badge_format(&ripr_bin, workspace_root, "repo-badge-shields")?;
    badge.label = "ripr+".to_string();
    Ok(badge)
}

fn run_ripr_badge_format(
    ripr_bin: &str,
    workspace_root: &Path,
    format: &str,
) -> Result<ShieldsEndpointBadge> {
    let output = Command::new(ripr_bin)
        .arg("check")
        .arg("--root")
        .arg(workspace_root)
        .arg("--mode")
        .arg("ready")
        .arg("--format")
        .arg(format)
        .current_dir(workspace_root)
        .output()
        .with_context(|| format!("spawning `{ripr_bin} check`"))?;

    if !output.status.success() {
        bail!(
            "{ripr_bin} {format} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{ripr_bin} emitted invalid Shields endpoint JSON"))
}

fn can_fallback_to_repo_badge(error: &str) -> bool {
    error.contains("test-efficiency.json") || error.contains("suppressions.toml validation failed")
}

fn write_json_pretty(path: &Path, badge: &ShieldsEndpointBadge) -> Result<()> {
    let mut json = serde_json::to_string_pretty(badge)
        .with_context(|| format!("serializing {}", path.display()))?;
    json.push('\n');
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
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

// ─── ripr review-comments ──────────────────────────────────────────────────

#[derive(Debug, clap::Args)]
pub struct ReviewCommentsArgs {
    /// PR base ref passed to `ripr review-comments`.
    #[arg(long, default_value = "origin/main")]
    pub base: String,

    /// PR head ref passed to `ripr review-comments`.
    #[arg(long, default_value = "HEAD")]
    pub head: String,

    /// Validate the review-comments output contract instead of running ripr.
    #[arg(long)]
    pub check: bool,
}

const REVIEW_COMMENTS_JSON: &str = "target/ripr/review/comments.json";
const REVIEW_COMMENTS_MD: &str = "target/ripr/review/comments.md";

pub fn ripr_review_comments(args: &ReviewCommentsArgs) -> Result<()> {
    let workspace_root = workspace_root()?;
    if args.check {
        return check_review_comments_contract(&workspace_root);
    }

    let ripr_bin = ripr_bin();
    let out = workspace_root.join(REVIEW_COMMENTS_JSON);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let status = Command::new(&ripr_bin)
        .arg("review-comments")
        .arg("--root")
        .arg(&workspace_root)
        .arg("--base")
        .arg(&args.base)
        .arg("--head")
        .arg(&args.head)
        .arg("--out")
        .arg(&out)
        .current_dir(&workspace_root)
        .status()
        .with_context(|| format!("spawning `{ripr_bin} review-comments`"))?;

    if !status.success() {
        eprintln!(
            "ripr review-comments exited with status {} — findings are advisory; see target/ripr/review/",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn check_review_comments_contract(workspace_root: &Path) -> Result<()> {
    validate_json_file(&workspace_root.join(REVIEW_COMMENTS_JSON))?;
    validate_nonempty_file(&workspace_root.join(REVIEW_COMMENTS_MD))?;
    println!("ripr-review-comments: required review guidance files are present");
    Ok(())
}

fn validate_json_file(path: &Path) -> Result<()> {
    let raw = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let _: Value = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing JSON from {}", path.display()))?;
    Ok(())
}

fn validate_nonempty_file(path: &Path) -> Result<()> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if raw.trim().is_empty() {
        bail!("{} is empty", path.display());
    }
    Ok(())
}

fn ripr_bin() -> String {
    std::env::var("RIPR_BIN").unwrap_or_else(|_| "ripr".to_string())
}

fn workspace_root() -> Result<PathBuf> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .context("spawning `cargo metadata` to locate workspace root")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let value: Value = serde_json::from_slice(&output.stdout).context("parsing cargo metadata")?;
    let root = value
        .get("workspace_root")
        .and_then(Value::as_str)
        .context("cargo metadata missing workspace_root")?;
    Ok(PathBuf::from(root))
}

#[cfg(test)]
fn shields_color(count: u64) -> &'static str {
    match count {
        0 => "brightgreen",
        1..=99 => "yellowgreen",
        100..=999 => "orange",
        _ => "red",
    }
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
    fn badge_plus_fallback_is_limited_to_known_auxiliary_input_gaps() {
        assert!(can_fallback_to_repo_badge(
            "missing target/ripr/reports/test-efficiency.json"
        ));
        assert!(can_fallback_to_repo_badge(
            "policy/ripr-suppressions.toml validation failed"
        ));
        assert!(!can_fallback_to_repo_badge(
            "ripr check crashed unexpectedly"
        ));
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
    fn write_shields_endpoint_shape() {
        // Round-trip via serde_json to confirm the four required keys are
        // present and that the `message` field remains a string.
        let dir = std::env::temp_dir().join("shipper-ripr-badge-test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("ripr-probe.json");
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
    }
}
