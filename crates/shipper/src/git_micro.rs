use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::types::GitContext;

/// Collect git context information for the current repository
/// Returns None if not in a git repository.
pub fn collect_git_context() -> Option<GitContext> {
    let repo_root = std::env::current_dir().ok()?;

    let git_program = git_program();
    if !is_repo_root(&repo_root, &git_program) {
        return None;
    }

    if env::var("SHIPPER_GIT_BIN").is_ok() {
        let commit = get_git_commit(&repo_root, &git_program);
        let branch = get_git_branch(&repo_root, &git_program);
        let tag = get_git_tag(&repo_root, &git_program);
        let dirty = get_git_dirty_status(&repo_root, &git_program);
        return Some(GitContext {
            commit,
            branch,
            tag,
            dirty,
        });
    }

    let git_context = shipper_git::get_git_context(&repo_root);
    if !shipper_git::is_git_repo(&repo_root) {
        return None;
    }

    Some(git_context)
}

/// Get the current commit SHA
fn get_git_commit(repo_root: &Path, git_program: &str) -> Option<String> {
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

/// Get the current branch name
fn get_git_branch(repo_root: &Path, git_program: &str) -> Option<String> {
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

/// Get the current tag (if any)
fn get_git_tag(repo_root: &Path, git_program: &str) -> Option<String> {
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

/// Check if the working tree is dirty
fn get_git_dirty_status(repo_root: &Path, git_program: &str) -> Option<bool> {
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

pub fn is_git_clean(repo_root: &Path) -> Result<bool> {
    if let Some(git_program) = env::var("SHIPPER_GIT_BIN").ok() {
        return local_is_git_clean(repo_root, &git_program);
    }

    shipper_git::is_git_clean(repo_root)
        .map_err(|err| anyhow::anyhow!("git status failed: {err}"))
}

fn local_is_git_clean(repo_root: &Path, git_program: &str) -> Result<bool> {
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

pub fn ensure_git_clean(repo_root: &Path) -> Result<()> {
    if !is_git_clean(repo_root)? {
        bail!("git working tree is not clean; commit/stash changes or use --allow-dirty");
    }
    Ok(())
}

fn is_repo_root(repo_root: &Path, git_program: &str) -> bool {
    Command::new(git_program)
        .arg("rev-parse")
        .arg("--git-dir")
        .current_dir(repo_root)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn git_program() -> String {
    env::var("SHIPPER_GIT_BIN").unwrap_or_else(|_| "git".to_string())
}
