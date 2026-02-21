//! Git operations for shipper.
//!
//! This crate provides git operations needed for publish verification
//! and context capture, including cleanliness checks and commit info.
//!
//! # Example
//!
//! ```
//! use shipper_git::{GitContext, is_git_clean, get_git_context};
//! use std::path::Path;
//!
//! // Check if the git working tree is clean
//! let clean = is_git_clean(Path::new(".")).unwrap_or(false);
//!
//! // Get git context for audit trail
//! let context = get_git_context(Path::new("."));
//! if let Some(commit) = context.commit {
//!     println!("Current commit: {}", commit);
//! }
//! ```

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Git context information for audit trail
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitContext {
    /// Current commit hash
    pub commit: Option<String>,
    /// Current branch name
    pub branch: Option<String>,
    /// Current tag (if on a tag)
    pub tag: Option<String>,
    /// Whether the working tree is dirty
    pub dirty: Option<bool>,
}

impl GitContext {
    /// Create a new empty git context
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if we have commit information
    pub fn has_commit(&self) -> bool {
        self.commit.is_some()
    }

    /// Check if the working tree is dirty
    pub fn is_dirty(&self) -> bool {
        self.dirty.unwrap_or(true)
    }

    /// Get a short commit hash (first 7 characters)
    pub fn short_commit(&self) -> Option<&str> {
        self.commit.as_ref().map(|c| {
            if c.len() > 7 {
                &c[..7]
            } else {
                c.as_str()
            }
        })
    }
}

/// Check if the git working tree is clean (no uncommitted changes)
pub fn is_git_clean(path: &Path) -> Result<bool> {
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

/// Check if we're inside a git repository
pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the current git commit hash
pub fn get_commit_hash(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .context("failed to run git rev-parse")?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(hash)
}

/// Get the current branch name
pub fn get_branch(path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .context("failed to run git rev-parse")?;

    if !output.status.success() {
        return Ok(None);
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    
    // If we're in detached HEAD state, return None
    if branch == "HEAD" {
        return Ok(None);
    }

    Ok(Some(branch))
}

/// Get the current tag (if on a tag)
pub fn get_tag(path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["describe", "--exact-match", "--tags"])
        .current_dir(path)
        .output()
        .context("failed to run git describe")?;

    if !output.status.success() {
        return Ok(None);
    }

    let tag = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(Some(tag))
}

/// Get complete git context
pub fn get_git_context(path: &Path) -> GitContext {
    let commit = get_commit_hash(path).ok();
    let branch = get_branch(path).ok().flatten();
    let tag = get_tag(path).ok().flatten();
    let dirty = is_git_clean(path).ok().map(|c| !c);

    GitContext {
        commit,
        branch,
        tag,
        dirty,
    }
}

/// Ensure git working tree is clean (returns error if dirty)
pub fn ensure_git_clean(path: &Path) -> Result<()> {
    if !is_git_clean(path)? {
        return Err(anyhow::anyhow!(
            "git working tree has uncommitted changes. Use --allow-dirty to bypass."
        ));
    }
    Ok(())
}

/// Check if a tag exists for the current commit
pub fn has_tag_for_commit(path: &Path) -> bool {
    get_tag(path).ok().flatten().is_some()
}

/// Get the list of changed files (staged + unstaged)
pub fn get_changed_files(path: &Path) -> Result<Vec<String>> {
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

    let status = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = status
        .lines()
        .map(|line| {
            // Format is "XY filename" - extract just the filename
            line.chars().skip(3).collect()
        })
        .collect();

    Ok(files)
}

/// Get remote URL for a given remote name
pub fn get_remote_url(path: &Path, remote: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["remote", "get-url", remote])
        .current_dir(path)
        .output()
        .context("failed to run git remote")?;

    if !output.status.success() {
        return Ok(None);
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(Some(url))
}

/// Check if we're on a specific branch
pub fn is_on_branch(path: &Path, branch_name: &str) -> bool {
    get_branch(path)
        .ok()
        .flatten()
        .map(|b| b == branch_name)
        .unwrap_or(false)
}

/// Check if the current commit is tagged
pub fn is_on_tag(path: &Path) -> bool {
    get_tag(path).ok().flatten().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::process::Command;

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
    fn is_git_repo_detects_repo() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        assert!(is_git_repo(td.path()));
    }

    #[test]
    fn is_git_repo_returns_false_for_non_repo() {
        let td = tempdir().expect("tempdir");
        assert!(!is_git_repo(td.path()));
    }

    #[test]
    fn is_git_clean_for_empty_repo() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        // Empty repo should be clean
        assert!(is_git_clean(td.path()).unwrap_or(false));
    }

    #[test]
    fn get_commit_hash_returns_hash() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        let hash = get_commit_hash(td.path()).expect("commit hash");
        assert_eq!(hash.len(), 40); // SHA-1 hash is 40 hex characters
    }

    #[test]
    fn get_branch_returns_branch() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        // After init, we might be on master or main
        let branch = get_branch(td.path()).expect("branch");
        // Could be "master", "main", or None depending on git version
        assert!(branch.is_none() || branch.as_ref().map_or(false, |b| b == "master" || b == "main"));
    }

    #[test]
    fn get_git_context_populates_fields() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        let context = get_git_context(td.path());

        assert!(context.has_commit());
        assert!(!context.is_dirty()); // Clean working tree
        assert!(context.short_commit().is_some());
    }

    #[test]
    fn git_context_default() {
        let context = GitContext::new();
        assert!(!context.has_commit());
        assert!(context.commit.is_none());
        assert!(context.branch.is_none());
    }

    #[test]
    fn short_commit_truncates() {
        let mut context = GitContext::new();
        context.commit = Some("0123456789abcdef0123456789abcdef01234567".to_string());

        assert_eq!(context.short_commit(), Some("0123456"));
    }

    #[test]
    fn ensure_git_clean_succeeds_when_clean() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        assert!(ensure_git_clean(td.path()).is_ok());
    }

    #[test]
    fn get_changed_files_empty_when_clean() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.is_empty());
    }

    #[test]
    fn get_remote_url_none_when_no_remote() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let url = get_remote_url(td.path(), "origin").expect("remote url");
        assert!(url.is_none());
    }
}