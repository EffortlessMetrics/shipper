//! `SHIPPER_GIT_BIN` override support.
//!
//! These helpers replicate the commit/branch/tag/dirty queries that live in
//! `super::context` but honor the `SHIPPER_GIT_BIN` environment variable so
//! tests (and sandboxed environments) can substitute a fake git binary.
//!
//! The override is set up by `collect_git_context` (in `super`) and by
//! `local_is_git_clean` (used by `super::cleanliness::is_git_clean`).
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command as StdCommand;

    use tempfile::TempDir;

    /// Path that is guaranteed not to exist as an executable on either Unix or
    /// Windows. Used to exercise every "command failed to spawn" branch.
    const NONEXISTENT_GIT: &str = "/definitely/not/a/real/git/binary/at/all";

    fn init_repo() -> TempDir {
        let dir = TempDir::new().expect("create tempdir for git repo");
        // `git init` configures HEAD; commit metadata setup below.
        run_git(&["init", "-q", "-b", "main"], dir.path());
        run_git(&["config", "user.name", "Test User"], dir.path());
        run_git(&["config", "user.email", "test@example.com"], dir.path());
        // First commit so HEAD has a SHA and rev-parse works.
        std::fs::write(dir.path().join("README"), b"hello").expect("write README");
        run_git(&["add", "README"], dir.path());
        run_git(
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"],
            dir.path(),
        );
        dir
    }

    fn run_git(args: &[&str], cwd: &Path) {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git binary available in test environment");
        assert!(
            out.status.success(),
            "git {args:?} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // NOTE: `git_program()` reads `SHIPPER_GIT_BIN` directly. Mutating the
    // process environment from a test would require `unsafe { env::set_var }`,
    // which the workspace forbids (`#![forbid(unsafe_code)]`). The downstream
    // helpers all accept `git_program: &str` as a parameter, which is the
    // logic we actually exercise below — covering both the override-success
    // path (passing `"git"`) and the spawn-failure path (passing a
    // nonexistent binary).

    // ---- is_repo_root() ----

    #[test]
    fn is_repo_root_true_for_initialized_repo() {
        let dir = init_repo();
        assert!(is_repo_root(dir.path(), "git"));
    }

    #[test]
    fn is_repo_root_false_for_non_repo_dir() {
        let dir = TempDir::new().expect("tempdir");
        assert!(!is_repo_root(dir.path(), "git"));
    }

    #[test]
    fn is_repo_root_false_when_binary_does_not_exist() {
        // spawn fails → mapped to false. Must not panic.
        let dir = TempDir::new().expect("tempdir");
        assert!(!is_repo_root(dir.path(), NONEXISTENT_GIT));
    }

    // ---- local_is_git_clean() ----

    #[test]
    fn local_is_git_clean_true_on_fresh_repo() {
        let dir = init_repo();
        assert!(local_is_git_clean(dir.path(), "git").expect("status succeeds"));
    }

    #[test]
    fn local_is_git_clean_false_with_untracked_file() {
        let dir = init_repo();
        std::fs::write(dir.path().join("dirty"), b"x").expect("write untracked file");
        assert!(!local_is_git_clean(dir.path(), "git").expect("status succeeds"));
    }

    #[test]
    fn local_is_git_clean_errors_when_binary_missing() {
        let dir = TempDir::new().expect("tempdir");
        let err = local_is_git_clean(dir.path(), NONEXISTENT_GIT)
            .expect_err("missing binary should surface an error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to execute git status"),
            "unexpected error chain: {msg}"
        );
    }

    #[test]
    fn local_is_git_clean_errors_when_git_status_nonzero() {
        // Run inside a non-repo directory: git status exits non-zero. The
        // module's contract is to bail with the historical "git status
        // failed:" prefix.
        let dir = TempDir::new().expect("tempdir");
        let err = local_is_git_clean(dir.path(), "git")
            .expect_err("non-repo dir should produce an error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("git status failed:"),
            "unexpected error chain (missing legacy prefix): {msg}"
        );
    }

    // ---- get_git_commit() ----

    #[test]
    fn get_git_commit_returns_ascii_sha_in_repo() {
        let dir = init_repo();
        let commit = get_git_commit(dir.path(), "git").expect("commit available after init");
        assert!(
            matches!(commit.len(), 40 | 64),
            "expected SHA-1 or SHA-256 hex object ID, got {commit:?}"
        );
        assert!(
            commit.chars().all(|c| c.is_ascii_hexdigit()),
            "non-hex chars in commit: {commit:?}"
        );
    }

    #[test]
    fn get_git_commit_none_in_non_repo_dir() {
        let dir = TempDir::new().expect("tempdir");
        assert!(get_git_commit(dir.path(), "git").is_none());
    }

    #[test]
    fn get_git_commit_none_when_binary_missing() {
        let dir = TempDir::new().expect("tempdir");
        assert!(get_git_commit(dir.path(), NONEXISTENT_GIT).is_none());
    }

    // ---- get_git_branch() ----

    #[test]
    fn get_git_branch_returns_branch_name_in_repo() {
        let dir = init_repo();
        let branch = get_git_branch(dir.path(), "git").expect("branch on initialized repo");
        assert_eq!(branch, "main");
    }

    #[test]
    fn get_git_branch_none_on_detached_head() {
        let dir = init_repo();
        // Detach by checking out the commit SHA directly.
        let sha = get_git_commit(dir.path(), "git").expect("commit");
        run_git(&["checkout", "-q", "--detach", &sha], dir.path());
        assert!(
            get_git_branch(dir.path(), "git").is_none(),
            "detached HEAD must surface as None (literal 'HEAD' rev-parse output)"
        );
    }

    #[test]
    fn get_git_branch_none_in_non_repo_dir() {
        let dir = TempDir::new().expect("tempdir");
        assert!(get_git_branch(dir.path(), "git").is_none());
    }

    #[test]
    fn get_git_branch_none_when_binary_missing() {
        let dir = TempDir::new().expect("tempdir");
        assert!(get_git_branch(dir.path(), NONEXISTENT_GIT).is_none());
    }

    // ---- get_git_tag() ----

    #[test]
    fn get_git_tag_some_when_head_is_tagged() {
        let dir = init_repo();
        run_git(&["-c", "tag.gpgSign=false", "tag", "v0.1.0"], dir.path());
        let tag = get_git_tag(dir.path(), "git").expect("tag at HEAD");
        assert_eq!(tag, "v0.1.0");
    }

    #[test]
    fn get_git_tag_none_when_no_tag_at_head() {
        let dir = init_repo();
        assert!(get_git_tag(dir.path(), "git").is_none());
    }

    #[test]
    fn get_git_tag_none_when_binary_missing() {
        let dir = TempDir::new().expect("tempdir");
        assert!(get_git_tag(dir.path(), NONEXISTENT_GIT).is_none());
    }

    // ---- get_git_dirty_status() ----

    #[test]
    fn get_git_dirty_status_false_on_clean_repo() {
        let dir = init_repo();
        assert_eq!(get_git_dirty_status(dir.path(), "git"), Some(false));
    }

    #[test]
    fn get_git_dirty_status_true_with_untracked_file() {
        let dir = init_repo();
        std::fs::write(dir.path().join("dirty"), b"x").expect("write file");
        assert_eq!(get_git_dirty_status(dir.path(), "git"), Some(true));
    }

    #[test]
    fn get_git_dirty_status_none_in_non_repo_dir() {
        let dir = TempDir::new().expect("tempdir");
        // status exits non-zero outside a repo ⇒ None.
        assert!(get_git_dirty_status(dir.path(), "git").is_none());
    }

    #[test]
    fn get_git_dirty_status_none_when_binary_missing() {
        let dir = TempDir::new().expect("tempdir");
        assert!(get_git_dirty_status(dir.path(), NONEXISTENT_GIT).is_none());
    }
}
