//! Git working-tree cleanliness checks.
//!
//! Two entry points:
//!
//! - [`is_git_clean`] — returns `Ok(true)` iff `git status --porcelain` yields no lines.
//! - [`ensure_git_clean`] — returns `Err` if the tree is dirty, with the
//!   historical "commit/stash changes or use --allow-dirty" message that the
//!   CLI snapshot tests pin.
//!
//! `SHIPPER_GIT_BIN` routing: when the env var is set, the override logic in
//! [`super::bin_override`] is used (so tests can point at a fake git). Otherwise
//! the default `git` binary is invoked directly.
//!
//! The default path wraps errors with a `git status failed:` prefix. The
//! shipper-cli snapshot tests assert against this phrasing, so do not change
//! the wording without updating the snapshots in `crates/shipper-cli/tests/snapshots/`.

use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use super::bin_override;

/// Check whether the git working tree is clean (no uncommitted changes).
///
/// When `SHIPPER_GIT_BIN` is set, routes through the override implementation in
/// [`super::bin_override::local_is_git_clean`]. Otherwise uses the default git
/// invocation. In both cases, an untracked file counts as "dirty".
pub fn is_git_clean(repo_root: &Path) -> Result<bool> {
    if let Ok(git_program) = env::var("SHIPPER_GIT_BIN") {
        return bin_override::local_is_git_clean(repo_root, &git_program);
    }

    is_git_clean_default(repo_root).map_err(|err| anyhow::anyhow!("git status failed: {err}"))
}

/// Default-path (no override) cleanliness check.
///
/// Preserved from the standalone `shipper-git` crate so that the error phrasing
/// is identical when no override is in effect. The outer wrapper in
/// [`is_git_clean`] adds an extra `git status failed:` prefix for CLI
/// backward-compatibility (see the module-level doc).
pub(super) fn is_git_clean_default(path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .context("failed to run git status")?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // If output is empty, the working tree is clean
    Ok(output.stdout.is_empty())
}

/// Fail fast if the working tree is dirty.
///
/// Error message is pinned by the CLI snapshot tests:
/// `"git working tree is not clean; commit/stash changes or use --allow-dirty"`.
pub fn ensure_git_clean(repo_root: &Path) -> Result<()> {
    if !is_git_clean(repo_root)? {
        anyhow::bail!("git working tree is not clean; commit/stash changes or use --allow-dirty");
    }
    Ok(())
}

/// Legacy error phrasing retained for snapshot compatibility.
///
/// The standalone `shipper-git` crate originally emitted
/// `"git working tree has uncommitted changes. Use --allow-dirty to bypass."`.
/// That exact string is pinned by an `insta` yaml snapshot
/// (`ensure_git_clean_error.snap`). This wrapper keeps that snapshot stable.
#[cfg(test)]
pub(super) fn ensure_git_clean_legacy(path: &Path) -> Result<()> {
    if !is_git_clean_default(path)? {
        return Err(anyhow::anyhow!(
            "git working tree has uncommitted changes. Use --allow-dirty to bypass."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn init_git_repo(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .expect("git init");

        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir)
            .output()
            .expect("git config");

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir)
            .output()
            .expect("git config");
    }

    fn make_commit(dir: &Path, msg: &str) {
        Command::new("git")
            .args(["commit", "--allow-empty", "-m", msg])
            .current_dir(dir)
            .output()
            .expect("git commit");
    }

    #[test]
    fn is_git_clean_for_empty_repo() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        // Empty repo should be clean
        assert!(is_git_clean_default(td.path()).unwrap_or(false));
    }

    #[test]
    fn is_git_clean_dirty_with_untracked_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("untracked.txt"), "hello").expect("write file");
        assert!(!is_git_clean_default(td.path()).expect("git status"));
    }

    #[test]
    fn is_git_clean_dirty_with_modified_tracked_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        // Create, add, and commit a file
        fs::write(td.path().join("file.txt"), "initial").expect("write file");
        Command::new("git")
            .args(["add", "."])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "initial");

        // Modify it
        fs::write(td.path().join("file.txt"), "modified").expect("write file");
        assert!(!is_git_clean_default(td.path()).expect("git status"));
    }

    #[test]
    fn ensure_git_clean_ok_on_clean_repo() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        assert!(ensure_git_clean_legacy(td.path()).is_ok());
    }

    #[test]
    fn ensure_git_clean_errors_with_allow_dirty_hint() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("dirty.txt"), "x").expect("write");
        let err = ensure_git_clean_legacy(td.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--allow-dirty"));
        assert!(msg.contains("uncommitted changes"));
    }

    #[test]
    fn ensure_git_clean_new_phrasing() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("dirty.txt"), "x").expect("write");
        let err = ensure_git_clean(td.path()).unwrap_err();
        let msg = err.to_string();
        // The new (canonical) phrasing used by the CLI.
        assert!(msg.contains("--allow-dirty"));
        assert!(msg.contains("not clean"));
    }
}
