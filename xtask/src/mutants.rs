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

    let changed = changed_rust_files(&args.base)?;
    let Some(cargo_arguments) = cargo_mutants_arguments(&changed, args.dry_run) else {
        println!(
            "no Rust source files changed vs {}; nothing to mutate.",
            args.base
        );
        return Ok(());
    };

    if !cargo_mutants_available() {
        println!("{CARGO_MUTANTS_INSTALL_HINT}");
        println!("`cargo xtask mutants-pr` exiting advisory-success (no cargo-mutants binary).");
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
    cmd.args(cargo_arguments);

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

fn cargo_mutants_arguments(changed: &[String], dry_run: bool) -> Option<Vec<String>> {
    if changed.is_empty() {
        // Never construct a workspace-wide cargo-mutants command without file
        // filters. This also keeps the no-Rust path ahead of the availability
        // probe, so an empty PR scope invokes no cargo-mutants process.
        return None;
    }
    let mut arguments = vec![
        "mutants".to_string(),
        "--no-shuffle".to_string(),
        // `--file` limits mutation to the PR's changed production files, but
        // cargo-mutants still discovers targets from Cargo's selected package
        // set. The workspace default members exclude crates such as
        // shipper-config, so select the workspace before applying file filters.
        "--workspace".to_string(),
    ];
    if dry_run {
        arguments.push("--list".to_string());
    }
    for file in changed {
        arguments.push("--file".to_string());
        arguments.push(file.clone());
    }
    Some(arguments)
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
    use anyhow::ensure;

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

    #[test]
    fn non_default_member_keeps_workspace_and_exact_file_scope() -> Result<()> {
        let file = "crates/shipper-config/src/runtime_options/registry.rs".to_string();
        let arguments = cargo_mutants_arguments(std::slice::from_ref(&file), true)
            .context("non-default member should produce a mutation command")?;

        ensure!(arguments.contains(&"--workspace".to_string()));
        ensure!(arguments.contains(&"--list".to_string()));
        ensure!(arguments.windows(2).any(|pair| pair == ["--file", &file]));
        ensure!(
            arguments.iter().filter(|arg| *arg == "--file").count() == 1,
            "changed-file filtering must stay exact"
        );
        Ok(())
    }

    #[test]
    fn default_member_keeps_workspace_and_exact_file_scope() -> Result<()> {
        let file = "crates/shipper-cli/src/lib.rs".to_string();
        let arguments = cargo_mutants_arguments(std::slice::from_ref(&file), false)
            .context("default member should produce a mutation command")?;

        ensure!(arguments.contains(&"--workspace".to_string()));
        ensure!(!arguments.contains(&"--list".to_string()));
        ensure!(arguments.windows(2).any(|pair| pair == ["--file", &file]));
        ensure!(
            arguments.iter().filter(|arg| *arg == "--file").count() == 1,
            "workspace selection must not broaden the file filter"
        );
        Ok(())
    }

    #[test]
    fn empty_changed_set_builds_no_cargo_mutants_command() -> Result<()> {
        let arguments = cargo_mutants_arguments(&[], false);

        ensure!(arguments.is_none());
        Ok(())
    }
}
