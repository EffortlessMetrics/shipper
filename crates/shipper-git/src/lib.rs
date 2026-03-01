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
        }
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
