//! `cargo xtask mutants-pr` — thin wrapper around `cargo-mutants` for the
//! PR-time targeted mutation lane (#182 PR 3).
//!
//! Mutation testing is the runtime backstop that ripr's static analysis
//! cannot replace; full mutation runs live in the weekly schedule. This
//! wrapper exists so a maintainer (or a label-gated CI run) can target
//! only the files a PR changes — keeping mutation off every PR's hot
//! path while still making it cheap to invoke when warranted.
//!
//! Behaviour:
//!
//! ```text
//!   --changed          (required) limit mutation to files modified
//!                      vs `<base>` (default `origin/main`)
//!   --base <REF>       diff base ref (default `origin/main`)
//!   --dry-run          enumerate the mutants `cargo mutants` would
//!                      generate without running tests against any of
//!                      them (maps to `cargo mutants --list`)
//! ```
//!
//! Local advisory: if `cargo-mutants` is missing on PATH, prints install
//! instructions and exits success. CI installs the tool before invoking.

use std::process::Command;

use anyhow::{Context, Result, bail};

const CARGO_MUTANTS_INSTALL_HINT: &str =
    "cargo-mutants not found. Install with: `cargo install cargo-mutants --locked`";

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Limit mutation to files modified vs `--base`. Currently the only
    /// supported scoping mode; passing the flag is required so the CLI
    /// shape stays explicit for the eventual addition of `--all` or a
    /// per-crate scope.
    #[arg(long)]
    pub changed: bool,

    /// Diff base ref. `cargo xtask mutants-pr --changed` computes the
    /// changed-file set as `git diff <base>...HEAD --name-only`.
    #[arg(long, default_value = "origin/main")]
    pub base: String,

    /// Enumerate the mutants `cargo mutants` would generate but do not
    /// run tests against any of them. Maps to `cargo mutants --list`.
    #[arg(long)]
    pub dry_run: bool,
}

pub fn mutants_pr(args: &Args) -> Result<()> {
    if !args.changed {
        bail!(
            "cargo xtask mutants-pr requires --changed today; bare invocation \
             would run cargo-mutants against the whole workspace which is \
             intentionally not part of the PR-time lane (see \
             docs/ci/test-evidence-lanes.md)."
        );
    }

    if !cargo_mutants_available() {
        println!("{CARGO_MUTANTS_INSTALL_HINT}");
        println!("`cargo xtask mutants-pr` exiting advisory-success (no cargo-mutants binary).");
        return Ok(());
    }

    let changed = changed_rust_files(&args.base)?;
    if changed.is_empty() {
        println!(
            "no Rust source files changed vs {}; nothing to mutate.",
            args.base
        );
        return Ok(());
    }

    println!(
        "cargo xtask mutants-pr --changed --base {} ({} files):",
        args.base,
        changed.len()
    );
    for f in &changed {
        println!("  {f}");
    }

    let mut cmd = Command::new("cargo");
    cmd.arg("mutants").arg("--no-shuffle");
    if args.dry_run {
        cmd.arg("--list");
    }
    for f in &changed {
        cmd.arg("--file").arg(f);
    }

    let status = cmd.status().context("spawning `cargo mutants`")?;
    if !status.success() {
        // cargo-mutants exits non-zero when surviving mutants are found.
        // Surface the exit code; the workflow's label gate keeps this off
        // the hot path, but when it does run we want the failure to be
        // load-bearing (unlike ripr, which is purely advisory).
        bail!("`cargo mutants` exited with status {}", status);
    }
    Ok(())
}

fn cargo_mutants_available() -> bool {
    Command::new("cargo")
        .args(["mutants", "--version"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn changed_rust_files(base: &str) -> Result<Vec<String>> {
    // `git diff <base>...HEAD --name-only -- '*.rs'` gives the files
    // changed on the current branch since it diverged from `base`. The
    // three-dot form keeps us from including files that changed on `base`
    // since the branch was cut.
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            &format!("{base}...HEAD"),
            "--",
            "*.rs",
        ])
        .output()
        .context("running `git diff`")?;
    if !output.status.success() {
        bail!(
            "`git diff {base}...HEAD` exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        // cargo-mutants only knows how to mutate Rust source files inside
        // a crate's compiled tree; integration tests under `tests/` are
        // excluded so we don't burn cycles trying to "mutate" assertions.
        .filter(|s| !s.contains("/tests/") && !s.contains("/benches/"))
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hint_mentions_cargo_install() {
        assert!(CARGO_MUTANTS_INSTALL_HINT.contains("cargo install cargo-mutants"));
        assert!(CARGO_MUTANTS_INSTALL_HINT.contains("--locked"));
    }

    #[test]
    fn args_defaults_are_explicit() {
        // Default base and dry-run flags must stay stable so the CI
        // invocation shape does not silently drift.
        use clap::Parser;
        #[derive(Parser, Debug)]
        struct Probe {
            #[command(flatten)]
            args: Args,
        }
        let parsed = Probe::parse_from(["probe", "--changed"]);
        assert!(parsed.args.changed);
        assert_eq!(parsed.args.base, "origin/main");
        assert!(!parsed.args.dry_run);
    }

    #[test]
    fn changed_requires_the_flag() {
        // Bare `cargo xtask mutants-pr` must refuse, since whole-workspace
        // mutation is intentionally off the PR-time lane.
        let args = Args {
            changed: false,
            base: "origin/main".to_string(),
            dry_run: false,
        };
        let err = mutants_pr(&args).unwrap_err();
        assert!(
            err.to_string().contains("requires --changed"),
            "unexpected error: {err}"
        );
    }
}
