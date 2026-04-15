//! `SHIPPER_GIT_BIN` override support.
//!
//! These helpers replicate the commit/branch/tag/dirty queries that live in
//! [`super::context`] but honor the `SHIPPER_GIT_BIN` environment variable so
//! tests (and sandboxed environments) can substitute a fake git binary.
//!
//! The override is set up by [`collect_git_context`] (in [`super::mod`]) and
//! [`local_is_git_clean`] (used by [`super::cleanliness::is_git_clean`]).
//!
//! Invariants:
//!
//! - When `SHIPPER_GIT_BIN` is set, the collector uses ONLY these helpers —
//!   it never falls back to the default `git` binary for any sub-query.
//! - `git_program()` returns the override value verbatim (including empty
//!   strings), matching the historical shim behavior from
//!   `shipper/src/git.rs` (pre-absorption).

use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Resolve the git program to invoke: `$SHIPPER_GIT_BIN` if set, else `"git"`.
pub(super) fn git_program() -> String {
    env::var("SHIPPER_GIT_BIN").unwrap_or_else(|_| "git".to_string())
}

/// Is this directory the root (or inside) a git repository?
///
/// Implemented via `git rev-parse --git-dir`, matching the historical shim.
pub(super) fn is_repo_root(repo_root: &Path, git_program: &str) -> bool {
    Command::new(git_program)
        .arg("rev-parse")
        .arg("--git-dir")
        .current_dir(repo_root)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Cleanliness check variant that honors the `SHIPPER_GIT_BIN` override.
///
/// Error text preserves the double `git status failed:` prefix that the CLI
/// snapshot tests assert against (see `cleanliness.rs` module doc).
pub(super) fn local_is_git_clean(repo_root: &Path, git_program: &str) -> Result<bool> {
    let out = Command::new(git_program)
        .arg("status")
        .arg("--porcelain")
        .current_dir(repo_root)
        .output()
        .context("failed to execute git status; is git installed?")?;

    if !out.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

/// Get the current commit SHA via the overridden git program.
pub(super) fn get_git_commit(repo_root: &Path, git_program: &str) -> Option<String> {
    let output = Command::new(git_program)
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo_root)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Get the current branch via the overridden git program.
///
/// Returns `None` for a detached HEAD (or any error). Matches the shim
/// behavior: `git rev-parse --abbrev-ref HEAD` → if output is literally
/// `HEAD`, report `None`.
pub(super) fn get_git_branch(repo_root: &Path, git_program: &str) -> Option<String> {
    let output = Command::new(git_program)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .current_dir(repo_root)
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch == "HEAD" { None } else { Some(branch) }
    } else {
        None
    }
}

/// Get the tag for the current commit via the overridden git program.
pub(super) fn get_git_tag(repo_root: &Path, git_program: &str) -> Option<String> {
    let output = Command::new(git_program)
        .arg("describe")
        .arg("--tags")
        .arg("--exact-match")
        .current_dir(repo_root)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Dirty-flag probe via the overridden git program.
pub(super) fn get_git_dirty_status(repo_root: &Path, git_program: &str) -> Option<bool> {
    let output = Command::new(git_program)
        .arg("status")
        .arg("--porcelain")
        .current_dir(repo_root)
        .output()
        .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        Some(!stdout.trim().is_empty())
    } else {
        None
    }
}
