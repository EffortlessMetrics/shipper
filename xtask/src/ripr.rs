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

use std::process::Command;

use anyhow::{Context, Result};

const RIPR_INSTALL_HINT: &str =
    "ripr not found on PATH. Install with: `cargo install ripr --locked --version 0.5.0`";

pub fn ripr_pr() -> Result<()> {
    if which_ripr().is_none() {
        // Local advisory: do not fail the developer's session if ripr isn't
        // installed. CI pre-installs a pinned version, so this branch is
        // for local-only invocations.
        println!("{RIPR_INSTALL_HINT}");
        println!("`cargo xtask ripr-pr` exiting advisory-success (no ripr binary).");
        return Ok(());
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
    // The wrapper is intentionally trivial enough that it's tested by the
    // CI lane invoking it end-to-end. A unit test would have to either mock
    // out Command (changing the wrapper's shape) or rely on the host having
    // ripr installed (non-portable). Keeping it untested is the right call
    // here — re-evaluate if the wrapper grows logic beyond "spawn + report".

    #[test]
    fn install_hint_mentions_pinned_version() {
        // Guard against a future bump to the install command that forgets
        // to update the user-facing hint.
        assert!(super::RIPR_INSTALL_HINT.contains("cargo install ripr"));
        assert!(super::RIPR_INSTALL_HINT.contains("--locked"));
        assert!(super::RIPR_INSTALL_HINT.contains("--version"));
    }

    #[test]
    fn install_hint_pinned_version_matches_workflow() {
        // The pinned version in the hint and in .github/workflows/ripr.yml
        // must stay in sync. The workflow file is read at test time so any
        // bump in one place flags the other. The hint wraps the command in
        // backticks, so trim non-version chars off the parsed tail.
        let workflow = include_str!("../../.github/workflows/ripr.yml");
        let tail = super::RIPR_INSTALL_HINT
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
}
