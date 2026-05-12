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

use anyhow::{Context, Result};

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
