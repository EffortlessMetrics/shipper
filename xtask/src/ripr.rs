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
use std::process::Command;

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
    /// PR base ref. Currently advisory only — `ripr pilot` operates on
    /// the working tree, not a diff. Kept on the CLI surface so the
    /// invocation shape ("`cargo xtask ripr-pr --base origin/main`")
    /// stays stable across future wrapper revisions.
    #[arg(long, default_value = "origin/main")]
    pub base: String,
}

pub fn ripr_pr(args: &Args) -> Result<()> {
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

    let status = Command::new("ripr")
        .args(["pilot", "--root", "."])
        .status()
        .context("spawning `ripr pilot --root .`")?;

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
    let status = Command::new("ripr").arg("--version").status();
    match status {
        Ok(s) if s.success() => Some(()),
        Ok(_) | Err(_) => None,
    }
}

// ─── repo-ripr-badge-artifacts ──────────────────────────────────────────────
//
// Repo-scoped Shields endpoint JSON for the public README badges. Per
// ripr's badge policy (docs/BADGE_POLICY.md upstream), README badges
// must be repo-scoped — a diff-scoped artifact would read `0` on `main`
// simply because nothing changed, not because the repo is clean.
//
// This command runs `ripr check --root . --mode ready --format
// repo-exposure-json`, captures the resulting repo summary, extracts
// `metrics.headline_eligible` (the count of repo seams the configured
// `[severity.seams]` policy treats as non-off), maps the number to a
// Shields color, and writes two Shields-compatible endpoint JSON files
// under `badges/`. Both badges currently project the same metric;
// `ripr+` is a forward-looking name kept aligned with upstream's pair.
// Differentiating it requires combining test-efficiency findings with
// exposure gaps and is deferred.

const SHIELDS_RIPR_PATH: &str = "badges/ripr.json";
const SHIELDS_RIPR_PLUS_PATH: &str = "badges/ripr-plus.json";
const RIPR_CHECK_REPO_OUT: &str = "target/ripr/check-repo.json";

pub fn repo_badge_artifacts() -> Result<()> {
    if which_ripr().is_none() {
        bail!(
            "ripr not found on PATH. Install with: `cargo install ripr --locked --version 0.5.0` \
             before regenerating badges."
        );
    }

    // `ripr check` streams its JSON output on stdout; capture it directly.
    let output = Command::new("ripr")
        .args([
            "check",
            "--root",
            ".",
            "--mode",
            "ready",
            "--format",
            "repo-exposure-json",
        ])
        .output()
        .context("spawning `ripr check`")?;
    if !output.status.success() {
        bail!(
            "`ripr check` exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    // Persist the raw repo-exposure JSON so the badge inputs are inspectable.
    if let Some(parent) = Path::new(RIPR_CHECK_REPO_OUT).parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(RIPR_CHECK_REPO_OUT, &output.stdout)
        .with_context(|| format!("writing {RIPR_CHECK_REPO_OUT}"))?;

    let value: Value =
        serde_json::from_slice(&output.stdout).context("parsing ripr repo-exposure JSON")?;
    let headline = value
        .get("metrics")
        .and_then(|m| m.get("headline_eligible"))
        .and_then(|v| v.as_u64())
        .context("`metrics.headline_eligible` missing from ripr repo-exposure JSON")?;

    fs::create_dir_all("badges").context("creating badges/")?;
    write_shields_endpoint(SHIELDS_RIPR_PATH, "ripr", headline)?;
    // `ripr+` upstream-aligned name. Pinned to the same count for now;
    // the differentiation between exposure-only (ripr) and exposure +
    // test-efficiency (ripr+) is upstream territory and not yet projected
    // here. Documented in `docs/ci/ripr.md`.
    write_shields_endpoint(SHIELDS_RIPR_PLUS_PATH, "ripr+", headline)?;

    println!(
        "repo-ripr-badge-artifacts: headline_eligible={headline} -> {SHIELDS_RIPR_PATH}, {SHIELDS_RIPR_PLUS_PATH}"
    );
    Ok(())
}

fn write_shields_endpoint(path: &str, label: &str, count: u64) -> Result<()> {
    let endpoint = serde_json::json!({
        "schemaVersion": 1,
        "label": label,
        "message": count.to_string(),
        "color": shields_color(count),
    });
    let mut s =
        serde_json::to_string_pretty(&endpoint).with_context(|| format!("serialising {path}"))?;
    s.push('\n');
    fs::write(path, s).with_context(|| format!("writing {path}"))
}

fn shields_color(count: u64) -> &'static str {
    // Color thresholds mirror ripr's own dogfood pattern: `0` is the
    // inbox-zero goal (brightgreen); any non-zero count is concern. The
    // exact thresholds will tune over time as Shipper's debt shrinks.
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
    fn write_shields_endpoint_shape() {
        // Round-trip via serde_json to confirm the four required keys are
        // present and that the count goes into the `message` field as a
        // string (Shields rejects numeric `message`).
        let dir = std::env::temp_dir().join("shipper-ripr-badge-test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("ripr-probe.json");
        write_shields_endpoint(path.to_str().unwrap(), "ripr", 42).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["label"], "ripr");
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
