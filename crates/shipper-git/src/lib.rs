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
        self.commit
            .as_ref()
            .map(|c| if c.len() > 7 { &c[..7] } else { c.as_str() })
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
    use std::fs;
    use std::process::Command;
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

    fn create_tag(dir: &Path, tag: &str) {
        Command::new("git")
            .args(["tag", tag])
            .current_dir(dir)
            .output()
            .expect("git tag");
    }

    fn add_remote(dir: &Path, name: &str, url: &str) {
        Command::new("git")
            .args(["remote", "add", name, url])
            .current_dir(dir)
            .output()
            .expect("git remote add");
    }

    // ── is_git_repo ──

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

    // ── is_git_clean / ensure_git_clean ──

    #[test]
    fn is_git_clean_for_empty_repo() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        // Empty repo should be clean
        assert!(is_git_clean(td.path()).unwrap_or(false));
    }

    #[test]
    fn is_git_clean_dirty_with_untracked_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("untracked.txt"), "hello").expect("write file");
        assert!(!is_git_clean(td.path()).expect("git status"));
    }

    #[test]
    fn is_git_clean_dirty_with_modified_tracked_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let file = td.path().join("tracked.txt");
        fs::write(&file, "original").expect("write");
        Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "add tracked");

        fs::write(&file, "modified").expect("modify");
        assert!(!is_git_clean(td.path()).expect("git status"));
    }

    #[test]
    fn is_git_clean_dirty_with_staged_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("staged.txt"), "content").expect("write");
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");

        assert!(!is_git_clean(td.path()).expect("git status"));
    }

    #[test]
    fn is_git_clean_errors_on_non_git_dir() {
        let td = tempdir().expect("tempdir");
        assert!(is_git_clean(td.path()).is_err());
    }

    #[test]
    fn ensure_git_clean_succeeds_when_clean() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        assert!(ensure_git_clean(td.path()).is_ok());
    }

    #[test]
    fn ensure_git_clean_fails_when_dirty() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("dirty.txt"), "dirt").expect("write");
        let err = ensure_git_clean(td.path());
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("uncommitted changes"));
    }

    // ── get_commit_hash ──

    #[test]
    fn get_commit_hash_returns_hash() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        let hash = get_commit_hash(td.path()).expect("commit hash");
        assert_eq!(hash.len(), 40); // SHA-1 hash is 40 hex characters
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn get_commit_hash_errors_without_commits() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        // No commits yet
        assert!(get_commit_hash(td.path()).is_err());
    }

    #[test]
    fn get_commit_hash_errors_on_non_git_dir() {
        let td = tempdir().expect("tempdir");
        assert!(get_commit_hash(td.path()).is_err());
    }

    #[test]
    fn get_commit_hash_changes_after_new_commit() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "first");
        let hash1 = get_commit_hash(td.path()).expect("hash1");

        make_commit(td.path(), "second");
        let hash2 = get_commit_hash(td.path()).expect("hash2");

        assert_ne!(hash1, hash2);
    }

    // ── get_branch ──

    #[test]
    fn get_branch_returns_branch() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        // After init, we might be on master or main
        let branch = get_branch(td.path()).expect("branch");
        // Could be "master", "main", or None depending on git version
        assert!(
            branch.is_none()
                || branch
                    .as_ref()
                    .is_some_and(|b| b == "master" || b == "main")
        );
    }

    #[test]
    fn get_branch_detects_custom_branch() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        Command::new("git")
            .args(["checkout", "-b", "feature/my-branch"])
            .current_dir(td.path())
            .output()
            .expect("git checkout");

        let branch = get_branch(td.path()).expect("branch").expect("some branch");
        assert_eq!(branch, "feature/my-branch");
    }

    #[test]
    fn get_branch_returns_none_for_detached_head() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let hash = get_commit_hash(td.path()).expect("hash");
        Command::new("git")
            .args(["checkout", &hash])
            .current_dir(td.path())
            .output()
            .expect("git checkout detached");

        let branch = get_branch(td.path()).expect("branch");
        assert!(branch.is_none());
    }

    // ── is_on_branch ──

    #[test]
    fn is_on_branch_matches_current_branch() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        Command::new("git")
            .args(["checkout", "-b", "release"])
            .current_dir(td.path())
            .output()
            .expect("git checkout");

        assert!(is_on_branch(td.path(), "release"));
        assert!(!is_on_branch(td.path(), "main"));
        assert!(!is_on_branch(td.path(), "master"));
    }

    #[test]
    fn is_on_branch_false_for_non_git_dir() {
        let td = tempdir().expect("tempdir");
        assert!(!is_on_branch(td.path(), "main"));
    }

    // ── Tag operations ──

    #[test]
    fn get_tag_returns_tag_on_tagged_commit() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "release");
        create_tag(td.path(), "v1.0.0");

        let tag = get_tag(td.path()).expect("get_tag").expect("tag present");
        assert_eq!(tag, "v1.0.0");
    }

    #[test]
    fn get_tag_returns_none_without_tag() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "no tag");

        let tag = get_tag(td.path()).expect("get_tag");
        assert!(tag.is_none());
    }

    #[test]
    fn get_tag_returns_none_after_moving_past_tag() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "tagged commit");
        create_tag(td.path(), "v0.1.0");
        make_commit(td.path(), "past the tag");

        let tag = get_tag(td.path()).expect("get_tag");
        assert!(tag.is_none());
    }

    #[test]
    fn has_tag_for_commit_true_when_tagged() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "tagged");
        create_tag(td.path(), "v2.0.0");

        assert!(has_tag_for_commit(td.path()));
    }

    #[test]
    fn has_tag_for_commit_false_when_not_tagged() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "no tag");

        assert!(!has_tag_for_commit(td.path()));
    }

    #[test]
    fn is_on_tag_true_when_tagged() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "tagged");
        create_tag(td.path(), "release-1");

        assert!(is_on_tag(td.path()));
    }

    #[test]
    fn is_on_tag_false_when_not_tagged() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "no tag");

        assert!(!is_on_tag(td.path()));
    }

    #[test]
    fn is_on_tag_false_for_non_git_dir() {
        let td = tempdir().expect("tempdir");
        assert!(!is_on_tag(td.path()));
    }

    // ── get_changed_files ──

    #[test]
    fn get_changed_files_empty_when_clean() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "test");

        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.is_empty());
    }

    #[test]
    fn get_changed_files_lists_untracked_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("new_file.txt"), "data").expect("write");
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(!files.is_empty());
        assert!(files.iter().any(|f| f.contains("new_file.txt")));
    }

    #[test]
    fn get_changed_files_lists_modified_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let file = td.path().join("file.txt");
        fs::write(&file, "v1").expect("write");
        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "add file");

        fs::write(&file, "v2").expect("modify");
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.iter().any(|f| f.contains("file.txt")));
    }

    #[test]
    fn get_changed_files_errors_on_non_git_dir() {
        let td = tempdir().expect("tempdir");
        assert!(get_changed_files(td.path()).is_err());
    }

    // ── get_remote_url ──

    #[test]
    fn get_remote_url_none_when_no_remote() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let url = get_remote_url(td.path(), "origin").expect("remote url");
        assert!(url.is_none());
    }

    #[test]
    fn get_remote_url_returns_configured_remote() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        add_remote(td.path(), "origin", "https://github.com/example/repo.git");

        let url = get_remote_url(td.path(), "origin")
            .expect("remote url")
            .expect("some url");
        assert_eq!(url, "https://github.com/example/repo.git");
    }

    #[test]
    fn get_remote_url_none_for_nonexistent_remote_name() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        add_remote(td.path(), "origin", "https://github.com/a/b.git");

        let url = get_remote_url(td.path(), "upstream").expect("remote url");
        assert!(url.is_none());
    }

    // ── GitContext unit tests ──

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
    fn short_commit_short_hash_returned_as_is() {
        let mut context = GitContext::new();
        context.commit = Some("abc".to_string());
        assert_eq!(context.short_commit(), Some("abc"));
    }

    #[test]
    fn short_commit_exactly_seven_chars() {
        let mut context = GitContext::new();
        context.commit = Some("abcdefg".to_string());
        assert_eq!(context.short_commit(), Some("abcdefg"));
    }

    #[test]
    fn short_commit_none_when_no_commit() {
        let context = GitContext::new();
        assert!(context.short_commit().is_none());
    }

    #[test]
    fn is_dirty_defaults_true_when_none() {
        let context = GitContext::new();
        assert!(context.is_dirty());
    }

    #[test]
    fn is_dirty_false_when_explicitly_clean() {
        let context = GitContext {
            dirty: Some(false),
            ..Default::default()
        };
        assert!(!context.is_dirty());
    }

    #[test]
    fn is_dirty_true_when_explicitly_dirty() {
        let context = GitContext {
            dirty: Some(true),
            ..Default::default()
        };
        assert!(context.is_dirty());
    }

    // ── get_git_context integration ──

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
    fn get_git_context_dirty_when_untracked_files() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("extra.txt"), "x").expect("write");
        let context = get_git_context(td.path());
        assert!(context.is_dirty());
        assert_eq!(context.dirty, Some(true));
    }

    #[test]
    fn get_git_context_includes_tag() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "tagged");
        create_tag(td.path(), "v3.0.0");

        let context = get_git_context(td.path());
        assert_eq!(context.tag.as_deref(), Some("v3.0.0"));
    }

    #[test]
    fn get_git_context_no_tag_when_untagged() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "no tag");

        let context = get_git_context(td.path());
        assert!(context.tag.is_none());
    }

    #[test]
    fn get_git_context_non_git_dir_returns_empty() {
        let td = tempdir().expect("tempdir");
        let context = get_git_context(td.path());
        assert!(!context.has_commit());
        assert!(context.branch.is_none());
        assert!(context.tag.is_none());
    }

    #[test]
    fn get_git_context_has_branch() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let context = get_git_context(td.path());
        assert!(context.branch.is_some());
    }

    // ── Serialization round-trip ──

    #[test]
    fn git_context_serde_round_trip() {
        let context = GitContext {
            commit: Some("abc123".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v1.0.0".to_string()),
            dirty: Some(false),
        };
        let json = serde_json::to_string(&context).expect("serialize");
        let deserialized: GitContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.commit.as_deref(), Some("abc123"));
        assert_eq!(deserialized.branch.as_deref(), Some("main"));
        assert_eq!(deserialized.tag.as_deref(), Some("v1.0.0"));
        assert_eq!(deserialized.dirty, Some(false));
    }

    #[test]
    fn git_context_serde_with_nones() {
        let context = GitContext::new();
        let json = serde_json::to_string(&context).expect("serialize");
        let deserialized: GitContext = serde_json::from_str(&json).expect("deserialize");
        assert!(deserialized.commit.is_none());
        assert!(deserialized.branch.is_none());
        assert!(deserialized.tag.is_none());
        assert!(deserialized.dirty.is_none());
    }

    // ── Property-based tests (proptest) ──

    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        fn arb_option_string() -> impl Strategy<Value = Option<String>> {
            prop_oneof![Just(None), ".*".prop_map(Some),]
        }

        fn arb_git_context() -> impl Strategy<Value = GitContext> {
            (
                arb_option_string(),
                arb_option_string(),
                arb_option_string(),
                prop_oneof![Just(None), any::<bool>().prop_map(Some)],
            )
                .prop_map(|(commit, branch, tag, dirty)| GitContext {
                    commit,
                    branch,
                    tag,
                    dirty,
                })
        }

        proptest! {
            // GitContext field values: has_commit iff commit is Some
            #[test]
            fn has_commit_iff_commit_is_some(ctx in arb_git_context()) {
                prop_assert_eq!(ctx.has_commit(), ctx.commit.is_some());
            }

            // is_dirty defaults to true when dirty is None, otherwise returns the inner value
            #[test]
            fn is_dirty_respects_field(dirty_opt in prop_oneof![Just(None), any::<bool>().prop_map(Some)]) {
                let ctx = GitContext { dirty: dirty_opt, ..Default::default() };
                let expected = dirty_opt.unwrap_or(true);
                prop_assert_eq!(ctx.is_dirty(), expected);
            }

            // short_commit truncates to 7 chars for realistic hex commit hashes
            #[test]
            fn short_commit_length(commit in "[0-9a-f]{1,40}") {
                let ctx = GitContext { commit: Some(commit.clone()), ..Default::default() };
                let short = ctx.short_commit().unwrap();
                if commit.len() > 7 {
                    prop_assert_eq!(short.len(), 7);
                    prop_assert_eq!(short, &commit[..7]);
                } else {
                    prop_assert_eq!(short, commit.as_str());
                }
            }

            // short_commit with arbitrary ASCII strings (safe for byte indexing)
            #[test]
            fn short_commit_ascii(commit in "[[:ascii:]]{1,80}") {
                let ctx = GitContext { commit: Some(commit.clone()), ..Default::default() };
                let short = ctx.short_commit().unwrap();
                if commit.len() > 7 {
                    prop_assert_eq!(short.len(), 7);
                } else {
                    prop_assert_eq!(short, commit.as_str());
                }
            }

            // short_commit is None when commit is None
            #[test]
            fn short_commit_none_when_no_commit(
                branch in arb_option_string(),
                tag in arb_option_string(),
                dirty in prop_oneof![Just(None), any::<bool>().prop_map(Some)],
            ) {
                let ctx = GitContext { commit: None, branch, tag, dirty };
                prop_assert!(ctx.short_commit().is_none());
            }

            // Porcelain line parsing: extracting filename by skipping first 3 chars
            #[test]
            fn porcelain_line_parsing(
                xy in "[MADRCU?! ]{2}",
                filename in "[a-zA-Z0-9_./-]+",
            ) {
                let line = format!("{} {}", xy, filename);
                let parsed: String = line.chars().skip(3).collect();
                prop_assert_eq!(parsed, filename);
            }

            // Porcelain parsing: empty output means no changed files
            #[test]
            fn porcelain_empty_output_means_clean(_dummy in 0..100u32) {
                let status = "";
                let files: Vec<String> = status
                    .lines()
                    .map(|line| line.chars().skip(3).collect())
                    .collect();
                prop_assert!(files.is_empty());
            }

            // Porcelain parsing: number of files matches number of lines
            #[test]
            fn porcelain_file_count_matches_lines(
                entries in prop::collection::vec(
                    ("[MADRCU?! ]{2}", "[a-zA-Z0-9_./]+"),
                    1..20,
                ),
            ) {
                let status: String = entries
                    .iter()
                    .map(|(xy, name)| format!("{} {}", xy, name))
                    .collect::<Vec<_>>()
                    .join("\n");
                let files: Vec<String> = status
                    .lines()
                    .map(|line| line.chars().skip(3).collect())
                    .collect();
                prop_assert_eq!(files.len(), entries.len());
            }

            // Serde round-trip preserves all fields for arbitrary contexts
            #[test]
            fn serde_round_trip_arbitrary(ctx in arb_git_context()) {
                let json = serde_json::to_string(&ctx).expect("serialize");
                let deserialized: GitContext = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(ctx.commit, deserialized.commit);
                prop_assert_eq!(ctx.branch, deserialized.branch);
                prop_assert_eq!(ctx.tag, deserialized.tag);
                prop_assert_eq!(ctx.dirty, deserialized.dirty);
            }

            // Debug output is non-empty and contains "GitContext"
            #[test]
            fn debug_output_valid(ctx in arb_git_context()) {
                let debug = format!("{:?}", ctx);
                prop_assert!(!debug.is_empty());
                prop_assert!(debug.contains("GitContext"));
            }

            // Clone produces identical context
            #[test]
            fn clone_is_identical(ctx in arb_git_context()) {
                let cloned = ctx.clone();
                prop_assert_eq!(ctx.commit, cloned.commit);
                prop_assert_eq!(ctx.branch, cloned.branch);
                prop_assert_eq!(ctx.tag, cloned.tag);
                prop_assert_eq!(ctx.dirty, cloned.dirty);
            }

            // Default context: all fields are None
            #[test]
            fn default_context_all_none(_dummy in 0..100u32) {
                let ctx = GitContext::default();
                prop_assert!(ctx.commit.is_none());
                prop_assert!(ctx.branch.is_none());
                prop_assert!(ctx.tag.is_none());
                prop_assert!(ctx.dirty.is_none());
                prop_assert!(!ctx.has_commit());
                prop_assert!(ctx.is_dirty()); // defaults to true
            }

            // is_clean (porcelain parsing) never panics for arbitrary status output
            #[test]
            fn is_clean_never_panics_for_arbitrary_output(output in "[ MADRCU?!]{0,3}[^\n]{0,200}(\n[ MADRCU?!]{0,3}[^\n]{0,200}){0,20}") {
                let is_clean = output.trim().is_empty();
                let files: Vec<String> = output
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|line| line.chars().skip(3).collect())
                    .collect();
                if is_clean {
                    prop_assert!(files.is_empty() || output.trim().is_empty());
                }
                // Main assertion: we got here without panicking
            }

            // Arbitrary file paths never cause panic in porcelain parsing
            #[test]
            fn porcelain_arbitrary_path_never_panics(
                path in "[^\0\n]{0,300}",
            ) {
                let line = format!("?? {}", path);
                let parsed: String = line.chars().skip(3).collect();
                prop_assert_eq!(parsed, path);
            }

            // Porcelain parsing handles lines shorter than 3 characters without panic
            #[test]
            fn porcelain_short_lines_never_panic(
                line in ".{0,3}",
            ) {
                let _parsed: String = line.chars().skip(3).collect();
                // If the line is < 3 chars, skip(3) just yields empty — no panic
            }
        }
    }

    // ── Edge-case: empty / uninitialized git repository ──

    #[test]
    fn uninitialized_dir_is_not_git_repo() {
        let td = tempdir().expect("tempdir");
        assert!(!is_git_repo(td.path()));
        assert!(get_commit_hash(td.path()).is_err());
        assert!(get_changed_files(td.path()).is_err());
    }

    #[test]
    fn git_init_only_is_repo_but_no_commits() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        assert!(is_git_repo(td.path()));
        // No commits yet — HEAD doesn't resolve
        assert!(get_commit_hash(td.path()).is_err());
        // Branch may be None or Some("master"/"main") depending on git version,
        // but should not error
        let branch = get_branch(td.path());
        assert!(branch.is_ok());
        // Tag should be None
        assert_eq!(get_tag(td.path()).unwrap(), None);
    }

    #[test]
    fn get_git_context_for_repo_with_no_commits() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let ctx = get_git_context(td.path());
        assert!(!ctx.has_commit());
        assert!(ctx.tag.is_none());
        assert!(ctx.short_commit().is_none());
    }

    // ── Edge-case: detached HEAD state ──

    #[test]
    fn detached_head_context_has_no_branch() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "first");
        let hash = get_commit_hash(td.path()).expect("hash");

        Command::new("git")
            .args(["checkout", &hash])
            .current_dir(td.path())
            .output()
            .expect("detach HEAD");

        let ctx = get_git_context(td.path());
        assert!(ctx.has_commit());
        assert!(ctx.branch.is_none());
        assert!(!ctx.is_dirty());
    }

    #[test]
    fn detached_head_is_on_branch_returns_false() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "first");
        let hash = get_commit_hash(td.path()).expect("hash");

        Command::new("git")
            .args(["checkout", &hash])
            .current_dir(td.path())
            .output()
            .expect("detach HEAD");

        assert!(!is_on_branch(td.path(), "main"));
        assert!(!is_on_branch(td.path(), "master"));
    }

    #[test]
    fn detached_head_with_dirty_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "first");
        let hash = get_commit_hash(td.path()).expect("hash");

        Command::new("git")
            .args(["checkout", &hash])
            .current_dir(td.path())
            .output()
            .expect("detach HEAD");

        fs::write(td.path().join("dirty.txt"), "content").expect("write");
        assert!(!is_git_clean(td.path()).unwrap());
        let ctx = get_git_context(td.path());
        assert!(ctx.is_dirty());
        assert!(ctx.branch.is_none());
    }

    // ── Edge-case: binary files and untracked directories ──

    #[test]
    fn status_with_binary_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        // Write a binary file (non-UTF-8 bytes)
        fs::write(
            td.path().join("image.png"),
            [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A],
        )
        .expect("write binary");
        assert!(!is_git_clean(td.path()).unwrap());
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.iter().any(|f| f.contains("image.png")));
    }

    #[test]
    fn status_with_untracked_directory() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let subdir = td.path().join("subdir");
        fs::create_dir(&subdir).expect("mkdir");
        fs::write(subdir.join("file.txt"), "content").expect("write");

        assert!(!is_git_clean(td.path()).unwrap());
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(!files.is_empty());
    }

    #[test]
    fn status_with_nested_untracked_directories() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let nested = td.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).expect("mkdir -p");
        fs::write(nested.join("deep.txt"), "deep").expect("write");

        assert!(!is_git_clean(td.path()).unwrap());
    }

    #[test]
    fn status_with_multiple_binary_files() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        for ext in &["png", "jpg", "exe", "bin"] {
            let name = format!("file.{ext}");
            fs::write(td.path().join(&name), [0xFF, 0xD8, 0x00]).expect("write binary");
        }
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.len() >= 4);
    }

    // ── Edge-case: submodules ──

    #[test]
    fn repo_with_submodule_init() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        // Create a second repo to use as submodule source
        let sub_src = tempdir().expect("submodule source");
        init_git_repo(sub_src.path());
        make_commit(sub_src.path(), "sub commit");

        let result = Command::new("git")
            .args([
                "submodule",
                "add",
                &sub_src.path().to_string_lossy(),
                "vendor/sub",
            ])
            .current_dir(td.path())
            .output();

        // Submodule add may fail on some CI setups; test what we can
        if let Ok(output) = result
            && output.status.success()
        {
            // The staged .gitmodules + submodule make the tree dirty
            assert!(!is_git_clean(td.path()).unwrap());
            let files = get_changed_files(td.path()).expect("changed files");
            assert!(!files.is_empty());
        }
    }

    // ── Edge-case: very long file paths ──

    #[test]
    fn status_with_long_file_path() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        // Create a deeply nested path (stay within OS limits)
        let mut path = td.path().to_path_buf();
        for i in 0..10 {
            path = path.join(format!("dir_{i:03}"));
        }
        fs::create_dir_all(&path).expect("create deep dirs");
        let long_name = "a".repeat(100) + ".txt";
        fs::write(path.join(&long_name), "content").expect("write");

        // Stage the file so porcelain shows the full path
        Command::new("git")
            .args(["add", "."])
            .current_dir(td.path())
            .output()
            .expect("git add");

        assert!(!is_git_clean(td.path()).unwrap());
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(!files.is_empty());
        assert!(files.iter().any(|f| f.contains(&long_name)));
    }

    #[test]
    fn changed_files_with_long_path_preserves_full_path() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let nested = td.path().join("very_long_directory_name_for_testing");
        fs::create_dir_all(&nested).expect("create dir");
        let fname = "b".repeat(80) + ".rs";
        fs::write(nested.join(&fname), "fn main() {}").expect("write");

        // Stage so porcelain shows file path, not just directory
        Command::new("git")
            .args(["add", "."])
            .current_dir(td.path())
            .output()
            .expect("git add");

        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.iter().any(|f| f.contains(&fname)));
    }

    // ── Edge-case: unicode file names ──

    #[test]
    fn status_with_unicode_filename() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        // Try creating a file with unicode characters
        let unicode_name = "café_résumé.txt";
        let write_result = fs::write(td.path().join(unicode_name), "unicode content");
        if write_result.is_ok() {
            assert!(!is_git_clean(td.path()).unwrap());
            let files = get_changed_files(td.path()).expect("changed files");
            assert!(!files.is_empty());
        }
    }

    #[test]
    fn status_with_cjk_filename() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let cjk_name = "日本語テスト.txt";
        let write_result = fs::write(td.path().join(cjk_name), "cjk content");
        if write_result.is_ok() {
            assert!(!is_git_clean(td.path()).unwrap());
        }
    }

    #[test]
    fn status_with_emoji_filename() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let emoji_name = "🚀launch.txt";
        let write_result = fs::write(td.path().join(emoji_name), "emoji content");
        if write_result.is_ok() {
            assert!(!is_git_clean(td.path()).unwrap());
        }
    }

    // ── Edge-case: deleted files ──

    #[test]
    fn status_with_deleted_tracked_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let file = td.path().join("to_delete.txt");
        fs::write(&file, "will be deleted").expect("write");
        Command::new("git")
            .args(["add", "to_delete.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "add file");

        fs::remove_file(&file).expect("delete file");
        assert!(!is_git_clean(td.path()).unwrap());
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.iter().any(|f| f.contains("to_delete.txt")));
    }

    // ── Edge-case: multiple tags on same commit ──

    #[test]
    fn multiple_tags_on_same_commit() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "tagged commit");
        create_tag(td.path(), "v1.0.0");
        create_tag(td.path(), "release-1.0.0");

        // get_tag returns one of them (git describe --exact-match picks one)
        let tag = get_tag(td.path()).expect("get_tag");
        assert!(tag.is_some());
    }

    // ── Edge-case: empty committed file ──

    #[test]
    fn empty_file_is_tracked_correctly() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        fs::write(td.path().join("empty.txt"), "").expect("write empty file");
        Command::new("git")
            .args(["add", "empty.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "add empty file");

        assert!(is_git_clean(td.path()).unwrap());
    }

    // ── Edge-case: ensure_git_clean on repo with no commits ──

    #[test]
    fn ensure_git_clean_on_empty_repo() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        // Empty repo (no commits) should be clean
        assert!(ensure_git_clean(td.path()).is_ok());
    }

    // ── Edge-case: remote URL with special characters ──

    #[test]
    fn remote_url_with_ssh_format() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        add_remote(td.path(), "origin", "git@github.com:user/repo.git");

        let url = get_remote_url(td.path(), "origin")
            .expect("remote url")
            .expect("some url");
        assert_eq!(url, "git@github.com:user/repo.git");
    }

    #[test]
    fn remote_url_with_token_in_https() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        add_remote(
            td.path(),
            "origin",
            "https://token:x-oauth-basic@github.com/user/repo.git",
        );

        let url = get_remote_url(td.path(), "origin")
            .expect("remote url")
            .expect("some url");
        assert!(url.contains("github.com/user/repo.git"));
    }

    // ── Hardened: staged AND unstaged simultaneously ──

    #[test]
    fn status_staged_and_unstaged_same_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let file = td.path().join("dual.txt");
        fs::write(&file, "v1").expect("write");
        Command::new("git")
            .args(["add", "dual.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "add dual");

        fs::write(&file, "v2").expect("write v2");
        Command::new("git")
            .args(["add", "dual.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add v2");
        fs::write(&file, "v3").expect("write v3");

        assert!(!is_git_clean(td.path()).unwrap());
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.iter().any(|f| f.contains("dual.txt")));
    }

    // ── Hardened: renamed file detection ──

    #[test]
    fn status_renamed_file() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        let file = td.path().join("old_name.txt");
        fs::write(&file, "content").expect("write");
        Command::new("git")
            .args(["add", "old_name.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "add file");

        Command::new("git")
            .args(["mv", "old_name.txt", "new_name.txt"])
            .current_dir(td.path())
            .output()
            .expect("git mv");

        assert!(!is_git_clean(td.path()).unwrap());
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(!files.is_empty());
    }

    // ── Hardened: branch with dots and slashes ──

    #[test]
    fn get_branch_with_dots_and_slashes() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        Command::new("git")
            .args(["checkout", "-b", "release/v2.0.0-rc.1"])
            .current_dir(td.path())
            .output()
            .expect("git checkout");

        let branch = get_branch(td.path()).expect("branch").expect("some branch");
        assert_eq!(branch, "release/v2.0.0-rc.1");
    }

    #[test]
    fn get_branch_deeply_nested_name() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        Command::new("git")
            .args(["checkout", "-b", "user/jdoe/fix/issue-42"])
            .current_dir(td.path())
            .output()
            .expect("git checkout");

        let branch = get_branch(td.path()).expect("branch").expect("some branch");
        assert_eq!(branch, "user/jdoe/fix/issue-42");
        assert!(is_on_branch(td.path(), "user/jdoe/fix/issue-42"));
        assert!(!is_on_branch(td.path(), "main"));
    }

    // ── Hardened: non-existent path ──

    #[test]
    fn is_git_clean_nonexistent_path() {
        let result = is_git_clean(Path::new("this/path/does/not/exist/at/all"));
        assert!(result.is_err());
    }

    #[test]
    fn get_commit_hash_nonexistent_path() {
        let result = get_commit_hash(Path::new("this/path/does/not/exist/at/all"));
        assert!(result.is_err());
    }

    #[test]
    fn get_changed_files_nonexistent_path() {
        let result = get_changed_files(Path::new("this/path/does/not/exist/at/all"));
        assert!(result.is_err());
    }

    // ── Hardened: clean after staging and committing ──

    #[test]
    fn is_git_clean_after_stage_and_commit() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("newfile.txt"), "data").expect("write");
        assert!(!is_git_clean(td.path()).unwrap());

        Command::new("git")
            .args(["add", "newfile.txt"])
            .current_dir(td.path())
            .output()
            .expect("git add");
        assert!(!is_git_clean(td.path()).unwrap());

        make_commit(td.path(), "commit newfile");
        assert!(is_git_clean(td.path()).unwrap());
    }

    // ── Hardened: multiple changed files parsing ──

    #[test]
    fn get_changed_files_multiple_mixed_statuses() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());

        fs::write(td.path().join("a.txt"), "a").expect("write");
        fs::write(td.path().join("b.txt"), "b").expect("write");
        Command::new("git")
            .args(["add", "."])
            .current_dir(td.path())
            .output()
            .expect("git add");
        make_commit(td.path(), "add files");

        fs::write(td.path().join("a.txt"), "a-modified").expect("write");
        fs::remove_file(td.path().join("b.txt")).expect("delete");
        fs::write(td.path().join("c.txt"), "c-new").expect("write");

        let files = get_changed_files(td.path()).expect("changed files");
        assert!(files.len() >= 3);
        assert!(files.iter().any(|f| f.contains("a.txt")));
        assert!(files.iter().any(|f| f.contains("b.txt")));
        assert!(files.iter().any(|f| f.contains("c.txt")));
    }

    // ── Hardened: context reflects correct dirty state transitions ──

    #[test]
    fn context_dirty_state_transitions() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        let ctx = get_git_context(td.path());
        assert!(!ctx.is_dirty());

        fs::write(td.path().join("file.txt"), "data").expect("write");
        let ctx = get_git_context(td.path());
        assert!(ctx.is_dirty());

        Command::new("git")
            .args(["add", "."])
            .current_dir(td.path())
            .output()
            .expect("git add");
        let ctx = get_git_context(td.path());
        assert!(ctx.is_dirty());

        make_commit(td.path(), "commit");
        let ctx = get_git_context(td.path());
        assert!(!ctx.is_dirty());
    }

    // ── Hardened: is_on_branch detached HEAD returns false for any name ──

    #[test]
    fn is_on_branch_false_for_detached_head() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "first");
        let hash = get_commit_hash(td.path()).expect("hash");

        Command::new("git")
            .args(["checkout", &hash])
            .current_dir(td.path())
            .output()
            .expect("detach HEAD");

        assert!(!is_on_branch(td.path(), "HEAD"));
        assert!(!is_on_branch(td.path(), "main"));
        assert!(!is_on_branch(td.path(), "master"));
        assert!(!is_on_branch(td.path(), ""));
    }

    // ── Hardened: has_commit and short_commit consistency ──

    #[test]
    fn short_commit_eight_chars_truncates() {
        let ctx = GitContext {
            commit: Some("abcdefgh".to_string()),
            ..Default::default()
        };
        assert!(ctx.has_commit());
        assert_eq!(ctx.short_commit(), Some("abcdefg"));
    }

    // ── Hardened: tag on earlier commit, branch moved ahead ──

    #[test]
    fn tag_not_visible_after_new_commit() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "tagged");
        create_tag(td.path(), "v0.9.0");
        assert!(has_tag_for_commit(td.path()));

        make_commit(td.path(), "past tag");
        assert!(!has_tag_for_commit(td.path()));

        let ctx = get_git_context(td.path());
        assert!(ctx.tag.is_none());
        assert!(ctx.has_commit());
    }

    // ── Hardened: get_remote_url on non-git dir ──

    #[test]
    fn get_remote_url_non_git_dir() {
        let td = tempdir().expect("tempdir");
        let result = get_remote_url(td.path(), "origin");
        assert!(result.is_err() || result.unwrap().is_none());
    }

    // ── Hardened: file with spaces in name ──

    #[test]
    fn status_with_spaces_in_filename() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("my file with spaces.txt"), "data").expect("write");
        assert!(!is_git_clean(td.path()).unwrap());
        let files = get_changed_files(td.path()).expect("changed files");
        assert!(!files.is_empty());
    }

    // ── Hardened: ensure_git_clean error message content ──

    #[test]
    fn ensure_git_clean_error_mentions_allow_dirty() {
        let td = tempdir().expect("tempdir");
        init_git_repo(td.path());
        make_commit(td.path(), "initial");

        fs::write(td.path().join("dirty.txt"), "x").expect("write");
        let err = ensure_git_clean(td.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--allow-dirty"));
        assert!(msg.contains("uncommitted changes"));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use insta::assert_yaml_snapshot;

    // ── GitContext data structure serialization ──

    #[test]
    fn git_context_full() {
        let ctx = GitContext {
            commit: Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v1.2.3".to_string()),
            dirty: Some(false),
        };
        assert_yaml_snapshot!(ctx);
    }

    #[test]
    fn git_context_empty() {
        let ctx = GitContext::new();
        assert_yaml_snapshot!(ctx);
    }

    #[test]
    fn git_context_commit_only() {
        let ctx = GitContext {
            commit: Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string()),
            branch: None,
            tag: None,
            dirty: None,
        };
        assert_yaml_snapshot!(ctx);
    }

    #[test]
    fn git_context_dirty_no_tag() {
        let ctx = GitContext {
            commit: Some("ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00".to_string()),
            branch: Some("feature/add-tests".to_string()),
            tag: None,
            dirty: Some(true),
        };
        assert_yaml_snapshot!(ctx);
    }

    // ── Cleanliness check result formats ──

    #[test]
    fn cleanliness_clean_context() {
        let ctx = GitContext {
            commit: Some("abc1234567890abc1234567890abc1234567890ab".to_string()),
            branch: Some("main".to_string()),
            tag: None,
            dirty: Some(false),
        };
        assert_yaml_snapshot!("clean_working_tree", ctx);
    }

    #[test]
    fn cleanliness_dirty_context() {
        let ctx = GitContext {
            commit: Some("abc1234567890abc1234567890abc1234567890ab".to_string()),
            branch: Some("main".to_string()),
            tag: None,
            dirty: Some(true),
        };
        assert_yaml_snapshot!("dirty_working_tree", ctx);
    }

    #[test]
    fn cleanliness_unknown_context() {
        let ctx = GitContext {
            commit: Some("abc1234567890abc1234567890abc1234567890ab".to_string()),
            branch: Some("main".to_string()),
            tag: None,
            dirty: None,
        };
        assert_yaml_snapshot!("unknown_dirty_state", ctx);
    }

    #[test]
    fn cleanliness_is_dirty_defaults_true() {
        let ctx = GitContext::new();
        // dirty=None => is_dirty() returns true
        #[derive(Serialize)]
        struct DirtyDefault {
            dirty_field: Option<bool>,
            is_dirty_result: bool,
        }
        let result = DirtyDefault {
            dirty_field: ctx.dirty,
            is_dirty_result: ctx.is_dirty(),
        };
        assert_yaml_snapshot!("dirty_default_behavior", result);
    }

    // ── Tag listing output formats ──

    #[test]
    fn tag_semver() {
        let ctx = GitContext {
            commit: Some("1111111111111111111111111111111111111111".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v2.0.0".to_string()),
            dirty: Some(false),
        };
        assert_yaml_snapshot!("tag_semver", ctx);
    }

    #[test]
    fn tag_prerelease() {
        let ctx = GitContext {
            commit: Some("2222222222222222222222222222222222222222".to_string()),
            branch: Some("release/v3".to_string()),
            tag: Some("v3.0.0-rc.1".to_string()),
            dirty: Some(false),
        };
        assert_yaml_snapshot!("tag_prerelease", ctx);
    }

    #[test]
    fn tag_absent() {
        let ctx = GitContext {
            commit: Some("3333333333333333333333333333333333333333".to_string()),
            branch: Some("develop".to_string()),
            tag: None,
            dirty: Some(false),
        };
        assert_yaml_snapshot!("tag_absent", ctx);
    }

    #[test]
    fn tag_with_dirty_tree() {
        let ctx = GitContext {
            commit: Some("4444444444444444444444444444444444444444".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v1.0.0".to_string()),
            dirty: Some(true),
        };
        assert_yaml_snapshot!("tag_dirty_tree", ctx);
    }

    // ── Error Display implementations ──

    #[test]
    fn error_ensure_git_clean_message() {
        let err = anyhow::anyhow!(
            "git working tree has uncommitted changes. Use --allow-dirty to bypass."
        );
        assert_yaml_snapshot!("ensure_git_clean_error", err.to_string());
    }

    #[test]
    fn error_git_status_failed() {
        let err = anyhow::anyhow!("git status failed: fatal: not a git repository");
        assert_yaml_snapshot!("git_status_failed_error", err.to_string());
    }

    #[test]
    fn error_git_rev_parse_failed() {
        let err = anyhow::anyhow!(
            "git rev-parse failed: fatal: ambiguous argument 'HEAD': unknown revision"
        );
        assert_yaml_snapshot!("git_rev_parse_failed_error", err.to_string());
    }

    // ── Short commit formatting ──

    #[test]
    fn short_commit_formats() {
        let cases = vec![
            ("full_hash", "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"),
            ("seven_chars", "abcdefg"),
            ("short_hash", "abc"),
        ];
        for (name, hash) in cases {
            let ctx = GitContext {
                commit: Some(hash.to_string()),
                ..Default::default()
            };
            assert_yaml_snapshot!(format!("short_commit_{name}"), ctx.short_commit());
        }
    }

    #[test]
    fn short_commit_none() {
        let ctx = GitContext::new();
        assert_yaml_snapshot!(ctx.short_commit());
    }
}

#[cfg(test)]
mod edge_case_snapshots {
    use super::*;
    use insta::assert_debug_snapshot;

    // ── Snapshot tests for GitContext variants ──

    #[test]
    fn snapshot_context_detached_head() {
        let ctx = GitContext {
            commit: Some("abc1234567890abc1234567890abc1234567890ab".to_string()),
            branch: None,
            tag: None,
            dirty: Some(false),
        };
        assert_debug_snapshot!("context_detached_head", ctx);
    }

    #[test]
    fn snapshot_context_dirty_detached() {
        let ctx = GitContext {
            commit: Some("ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00".to_string()),
            branch: None,
            tag: None,
            dirty: Some(true),
        };
        assert_debug_snapshot!("context_dirty_detached", ctx);
    }

    #[test]
    fn snapshot_context_no_commit_no_branch() {
        let ctx = GitContext::new();
        assert_debug_snapshot!("context_no_commit_no_branch", ctx);
    }

    #[test]
    fn snapshot_context_tagged_detached() {
        let ctx = GitContext {
            commit: Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string()),
            branch: None,
            tag: Some("v1.0.0".to_string()),
            dirty: Some(false),
        };
        assert_debug_snapshot!("context_tagged_detached", ctx);
    }

    #[test]
    fn snapshot_context_all_fields_populated() {
        let ctx = GitContext {
            commit: Some("aabbccddee1122334455aabbccddee1122334455".to_string()),
            branch: Some("release/v2.0".to_string()),
            tag: Some("v2.0.0-rc.1".to_string()),
            dirty: Some(false),
        };
        assert_debug_snapshot!("context_all_fields", ctx);
    }

    #[test]
    fn snapshot_context_dirty_with_tag() {
        let ctx = GitContext {
            commit: Some("5555555555555555555555555555555555555555".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v0.1.0".to_string()),
            dirty: Some(true),
        };
        assert_debug_snapshot!("context_dirty_with_tag", ctx);
    }

    // ── Snapshot tests for cleanliness check results ──

    #[test]
    fn snapshot_clean_result_ok() {
        // Simulates what ensure_git_clean returns on a clean repo
        let result: Result<(), String> = Ok(());
        assert_debug_snapshot!("clean_result_ok", result);
    }

    #[test]
    fn snapshot_clean_result_err_uncommitted() {
        let result: Result<(), String> = Err(
            "git working tree has uncommitted changes. Use --allow-dirty to bypass.".to_string(),
        );
        assert_debug_snapshot!("clean_result_err_uncommitted", result);
    }

    #[test]
    fn snapshot_clean_result_err_not_git() {
        let result: Result<(), String> =
            Err("git status failed: fatal: not a git repository".to_string());
        assert_debug_snapshot!("clean_result_err_not_git", result);
    }

    // ── Snapshot tests for short_commit edge cases ──

    #[test]
    fn snapshot_short_commit_empty_string() {
        let ctx = GitContext {
            commit: Some(String::new()),
            ..Default::default()
        };
        assert_debug_snapshot!("short_commit_empty_string", ctx.short_commit());
    }

    #[test]
    fn snapshot_short_commit_exactly_eight() {
        let ctx = GitContext {
            commit: Some("abcdefgh".to_string()),
            ..Default::default()
        };
        assert_debug_snapshot!("short_commit_exactly_eight", ctx.short_commit());
    }

    // ── Snapshot tests for is_dirty with all variants ──

    #[test]
    fn snapshot_is_dirty_variants() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct DirtyVariant {
            input: Option<bool>,
            result: bool,
        }
        let variants = vec![
            DirtyVariant {
                input: None,
                result: GitContext {
                    dirty: None,
                    ..Default::default()
                }
                .is_dirty(),
            },
            DirtyVariant {
                input: Some(true),
                result: GitContext {
                    dirty: Some(true),
                    ..Default::default()
                }
                .is_dirty(),
            },
            DirtyVariant {
                input: Some(false),
                result: GitContext {
                    dirty: Some(false),
                    ..Default::default()
                }
                .is_dirty(),
            },
        ];
        assert_debug_snapshot!("is_dirty_all_variants", variants);
    }

    // ── Snapshot: changed files output format ──

    #[test]
    fn snapshot_porcelain_parse_various_statuses() {
        let porcelain_output = "\
?? untracked.txt\n\
 M modified.txt\n\
A  staged_new.txt\n\
D  deleted.txt\n\
MM both_staged_and_modified.txt\n\
R  renamed.txt -> new_name.txt";

        let files: Vec<String> = porcelain_output
            .lines()
            .map(|line| line.chars().skip(3).collect())
            .collect();
        assert_debug_snapshot!("porcelain_parsed_files", files);
    }

    #[test]
    fn snapshot_porcelain_empty() {
        let porcelain_output = "";
        let files: Vec<String> = porcelain_output
            .lines()
            .map(|line| line.chars().skip(3).collect())
            .collect();
        assert_debug_snapshot!("porcelain_empty_output", files);
    }

    // ── Hardened snapshot: porcelain with renamed entries ──

    #[test]
    fn snapshot_porcelain_renamed_and_copied() {
        let porcelain_output = "\
R  old.txt -> new.txt\n\
C  original.txt -> copy.txt\n\
?? brand_new.txt";

        let files: Vec<String> = porcelain_output
            .lines()
            .map(|line| line.chars().skip(3).collect())
            .collect();
        assert_debug_snapshot!("porcelain_renamed_and_copied", files);
    }

    // ── Hardened snapshot: context on feature branch ──

    #[test]
    fn snapshot_context_feature_branch_clean() {
        let ctx = GitContext {
            commit: Some("aabbccddaabbccddaabbccddaabbccddaabbccdd".to_string()),
            branch: Some("feature/add-logging".to_string()),
            tag: None,
            dirty: Some(false),
        };
        assert_debug_snapshot!("context_feature_branch_clean", ctx);
    }

    // ── Hardened snapshot: porcelain with spaces in filenames ──

    #[test]
    fn snapshot_porcelain_spaces_in_names() {
        let porcelain_output = "\
?? my file.txt\n\
 M path with spaces/file.rs\n\
A  \"quoted name.txt\"";

        let files: Vec<String> = porcelain_output
            .lines()
            .map(|line| line.chars().skip(3).collect())
            .collect();
        assert_debug_snapshot!("porcelain_spaces_in_names", files);
    }
}
