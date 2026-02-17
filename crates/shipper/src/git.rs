use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::types::GitContext;

/// Collect git context information for the current repository
/// Returns None if not in a git repository
pub fn collect_git_context() -> Option<GitContext> {
    let repo_root = std::env::current_dir().ok()?;

    // Check if we're in a git repository
    let git_dir_check = Command::new(git_program())
        .arg("rev-parse")
        .arg("--git-dir")
        .current_dir(&repo_root)
        .output()
        .ok()?;

    if !git_dir_check.status.success() {
        return None;
    }

    // Get current commit SHA
    let commit = get_git_commit(&repo_root);

    // Get current branch name
    let branch = get_git_branch(&repo_root);

    // Get current tag (if any)
    let tag = get_git_tag(&repo_root);

    // Check for dirty working tree
    let dirty = get_git_dirty_status(&repo_root);

    Some(GitContext {
        commit,
        branch,
        tag,
        dirty,
    })
}

/// Get the current commit SHA
fn get_git_commit(repo_root: &Path) -> Option<String> {
    let output = Command::new(git_program())
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
fn get_git_branch(repo_root: &Path) -> Option<String> {
    let output = Command::new(git_program())
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .current_dir(repo_root)
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Filter out detached HEAD state
        if branch == "HEAD" { None } else { Some(branch) }
    } else {
        None
    }
}

/// Get the current tag (if any)
fn get_git_tag(repo_root: &Path) -> Option<String> {
    let output = Command::new(git_program())
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
fn get_git_dirty_status(repo_root: &Path) -> Option<bool> {
    let output = Command::new(git_program())
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
    let out = Command::new(git_program())
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

fn git_program() -> String {
    env::var("SHIPPER_GIT_BIN").unwrap_or_else(|_| "git".to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

    fn write_fake_git(bin_dir: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let path = bin_dir.join("git.cmd");
            fs::write(
                &path,
                "@echo off\r\nif \"%SHIPPER_GIT_MODE%\"==\"clean\" (\r\n  exit /b 0\r\n)\r\nif \"%SHIPPER_GIT_MODE%\"==\"dirty\" (\r\n  echo M src/lib.rs\r\n  exit /b 0\r\n)\r\necho fatal: mock failure 1>&2\r\nexit /b 1\r\n",
            )
            .expect("write fake git");
            path
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"$SHIPPER_GIT_MODE\" = \"clean\" ]; then\n  exit 0\nfi\nif [ \"$SHIPPER_GIT_MODE\" = \"dirty\" ]; then\n  echo 'M src/lib.rs'\n  exit 0\nfi\necho 'fatal: mock failure' >&2\nexit 1\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
            path
        }
    }

    fn system_git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn run_system_git(repo_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[serial]
    fn is_git_clean_true_when_porcelain_empty() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);

        temp_env::with_vars(
            [
                ("SHIPPER_GIT_BIN", Some(fake_git.to_str().expect("utf8"))),
                ("SHIPPER_GIT_MODE", Some("clean")),
            ],
            || {
                let ok = is_git_clean(td.path()).expect("git clean");
                assert!(ok);
            },
        );
    }

    #[test]
    #[serial]
    fn is_git_clean_false_when_porcelain_has_changes() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);

        temp_env::with_vars(
            [
                ("SHIPPER_GIT_BIN", Some(fake_git.to_str().expect("utf8"))),
                ("SHIPPER_GIT_MODE", Some("dirty")),
            ],
            || {
                let ok = is_git_clean(td.path()).expect("git clean");
                assert!(!ok);
            },
        );
    }

    #[test]
    #[serial]
    fn is_git_clean_surfaces_git_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);

        temp_env::with_vars(
            [
                ("SHIPPER_GIT_BIN", Some(fake_git.to_str().expect("utf8"))),
                ("SHIPPER_GIT_MODE", Some("fail")),
            ],
            || {
                let err = is_git_clean(td.path()).expect_err("must fail");
                assert!(format!("{err:#}").contains("git status failed"));
            },
        );
    }

    #[test]
    #[serial]
    fn ensure_git_clean_errors_for_dirty_tree() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);

        temp_env::with_vars(
            [
                ("SHIPPER_GIT_BIN", Some(fake_git.to_str().expect("utf8"))),
                ("SHIPPER_GIT_MODE", Some("dirty")),
            ],
            || {
                let err = ensure_git_clean(td.path()).expect_err("must fail");
                assert!(format!("{err:#}").contains("git working tree is not clean"));
            },
        );
    }

    #[test]
    #[serial]
    fn collect_git_context_returns_none_outside_git_repo() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);

        temp_env::with_vars(
            [
                ("SHIPPER_GIT_BIN", Some(fake_git.to_str().expect("utf8"))),
                ("SHIPPER_GIT_MODE", Some("fail")),
            ],
            || {
                let context = collect_git_context();
                assert!(context.is_none());
            },
        );
    }

    #[test]
    #[serial]
    fn collect_git_context_returns_some_in_git_repo() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");

        // Create a fake git that simulates a clean git repository
        #[cfg(windows)]
        {
            let path = bin.join("git.cmd");
            fs::write(
                &path,
                "@echo off\r\nif not \"%1\"==\"rev-parse\" goto :not_revparse\r\nif \"%2\"==\"--git-dir\" exit /b 0\r\nif \"%2\"==\"HEAD\" (\r\n  echo abc123def456\r\n  exit /b 0\r\n)\r\nif \"%2\"==\"--abbrev-ref\" (\r\n  echo main\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n:not_revparse\r\nif \"%1\"==\"describe\" exit /b 1\r\nif \"%1\"==\"status\" exit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"$1\" = \"rev-parse\" ]; then\n  if [ \"$2\" = \"--git-dir\" ]; then\n    exit 0\n  fi\n  if [ \"$2\" = \"HEAD\" ]; then\n    echo \"abc123def456\"\n    exit 0\n  fi\n  if [ \"$2\" = \"--abbrev-ref\" ]; then\n    echo \"main\"\n    exit 0\n  fi\nfi\nif [ \"$1\" = \"describe\" ]; then\n  exit 1\nfi\nif [ \"$1\" = \"status\" ]; then\n  exit 0\nfi\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }

        let git_bin_path = bin
            .join(if cfg!(windows) { "git.cmd" } else { "git" })
            .to_str()
            .expect("utf8")
            .to_string();

        temp_env::with_var("SHIPPER_GIT_BIN", Some(&git_bin_path), || {
            let context = collect_git_context();
            assert!(context.is_some());

            let ctx = context.unwrap();
            assert_eq!(ctx.commit, Some("abc123def456".to_string()));
            assert_eq!(ctx.branch, Some("main".to_string()));
            assert_eq!(ctx.tag, None);
            assert_eq!(ctx.dirty, Some(false));
        });
    }

    #[test]
    #[serial]
    fn collect_git_context_returns_dirty_status() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");

        // Create a fake git that simulates a dirty git repository
        #[cfg(windows)]
        {
            let path = bin.join("git.cmd");
            fs::write(
                &path,
                "@echo off\r\nif not \"%1\"==\"rev-parse\" goto :not_revparse\r\nif \"%2\"==\"--git-dir\" exit /b 0\r\nif \"%2\"==\"HEAD\" (\r\n  echo abc123def456\r\n  exit /b 0\r\n)\r\nif \"%2\"==\"--abbrev-ref\" (\r\n  echo main\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n:not_revparse\r\nif \"%1\"==\"describe\" exit /b 1\r\nif \"%1\"==\"status\" echo M src/lib.rs\r\nexit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"$1\" = \"rev-parse\" ]; then\n  if [ \"$2\" = \"--git-dir\" ]; then\n    exit 0\n  fi\n  if [ \"$2\" = \"HEAD\" ]; then\n    echo \"abc123def456\"\n    exit 0\n  fi\n  if [ \"$2\" = \"--abbrev-ref\" ]; then\n    echo \"main\"\n    exit 0\n  fi\nfi\nif [ \"$1\" = \"describe\" ]; then\n  exit 1\nfi\nif [ \"$1\" = \"status\" ]; then\n  echo \"M src/lib.rs\"\n  exit 0\nfi\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }

        let git_bin_path = bin
            .join(if cfg!(windows) { "git.cmd" } else { "git" })
            .to_str()
            .expect("utf8")
            .to_string();

        temp_env::with_var("SHIPPER_GIT_BIN", Some(&git_bin_path), || {
            let context = collect_git_context();
            assert!(context.is_some());

            let ctx = context.unwrap();
            assert_eq!(ctx.commit, Some("abc123def456".to_string()));
            assert_eq!(ctx.branch, Some("main".to_string()));
            assert_eq!(ctx.tag, None);
            assert_eq!(ctx.dirty, Some(true));
        });
    }

    #[test]
    #[serial]
    fn collect_git_context_returns_tag() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");

        // Create a fake git that simulates a git repository with a tag
        #[cfg(windows)]
        {
            let path = bin.join("git.cmd");
            fs::write(
                &path,
                "@echo off\r\nif not \"%1\"==\"rev-parse\" goto :not_revparse\r\nif \"%2\"==\"--git-dir\" exit /b 0\r\nif \"%2\"==\"HEAD\" (\r\n  echo abc123def456\r\n  exit /b 0\r\n)\r\nif \"%2\"==\"--abbrev-ref\" (\r\n  echo main\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n:not_revparse\r\nif not \"%1\"==\"describe\" goto :not_describe\r\nif not \"%4\"==\"\" exit /b 1\r\nif \"%2\"==\"--tags\" if \"%3\"==\"--exact-match\" (\r\n  echo v1.0.0\r\n  exit /b 0\r\n)\r\nexit /b 1\r\n:not_describe\r\nif \"%1\"==\"status\" exit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"$1\" = \"rev-parse\" ]; then\n  if [ \"$2\" = \"--git-dir\" ]; then\n    exit 0\n  fi\n  if [ \"$2\" = \"HEAD\" ]; then\n    echo \"abc123def456\"\n    exit 0\n  fi\n  if [ \"$2\" = \"--abbrev-ref\" ]; then\n    echo \"main\"\n    exit 0\n  fi\nfi\nif [ \"$1\" = \"describe\" ]; then\n  if [ \"$2\" = \"--tags\" ] && [ \"$3\" = \"--exact-match\" ] && [ -z \"$4\" ]; then\n    echo \"v1.0.0\"\n    exit 0\n  fi\n  exit 1\nfi\nif [ \"$1\" = \"status\" ]; then\n  exit 0\nfi\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }

        let git_bin_path = bin
            .join(if cfg!(windows) { "git.cmd" } else { "git" })
            .to_str()
            .expect("utf8")
            .to_string();

        temp_env::with_var("SHIPPER_GIT_BIN", Some(&git_bin_path), || {
            let context = collect_git_context();
            assert!(context.is_some());

            let ctx = context.unwrap();
            assert_eq!(ctx.commit, Some("abc123def456".to_string()));
            assert_eq!(ctx.branch, Some("main".to_string()));
            assert_eq!(ctx.tag, Some("v1.0.0".to_string()));
            assert_eq!(ctx.dirty, Some(false));
        });
    }

    #[test]
    #[serial]
    fn given_real_tagged_repo_when_get_git_tag_then_returns_exact_tag() {
        if !system_git_available() {
            return;
        }

        let td = tempdir().expect("tempdir");
        temp_env::with_var("SHIPPER_GIT_BIN", None::<&str>, || {
            run_system_git(td.path(), &["init"]);
            run_system_git(td.path(), &["config", "user.email", "shipper@example.test"]);
            run_system_git(td.path(), &["config", "user.name", "Shipper Test"]);

            fs::write(td.path().join("README.md"), "demo\n").expect("write");
            run_system_git(td.path(), &["add", "."]);
            run_system_git(td.path(), &["commit", "-m", "initial"]);
            run_system_git(td.path(), &["tag", "v1.2.3"]);

            let tag = get_git_tag(td.path());
            assert_eq!(tag.as_deref(), Some("v1.2.3"));
        });
    }

    #[test]
    #[serial]
    fn collect_git_context_handles_detached_head() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");

        // Create a fake git that simulates a detached HEAD state
        #[cfg(windows)]
        {
            let path = bin.join("git.cmd");
            fs::write(
                &path,
                "@echo off\r\nif not \"%1\"==\"rev-parse\" goto :not_revparse\r\nif \"%2\"==\"--git-dir\" exit /b 0\r\nif \"%2\"==\"HEAD\" (\r\n  echo abc123def456\r\n  exit /b 0\r\n)\r\nif \"%2\"==\"--abbrev-ref\" (\r\n  echo HEAD\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n:not_revparse\r\nif \"%1\"==\"describe\" exit /b 1\r\nif \"%1\"==\"status\" exit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"$1\" = \"rev-parse\" ]; then\n  if [ \"$2\" = \"--git-dir\" ]; then\n    exit 0\n  fi\n  if [ \"$2\" = \"HEAD\" ]; then\n    echo \"abc123def456\"\n    exit 0\n  fi\n  if [ \"$2\" = \"--abbrev-ref\" ]; then\n    echo \"HEAD\"\n    exit 0\n  fi\nfi\nif [ \"$1\" = \"describe\" ]; then\n  exit 1\nfi\nif [ \"$1\" = \"status\" ]; then\n  exit 0\nfi\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }

        let git_bin_path = bin
            .join(if cfg!(windows) { "git.cmd" } else { "git" })
            .to_str()
            .expect("utf8")
            .to_string();

        temp_env::with_var("SHIPPER_GIT_BIN", Some(&git_bin_path), || {
            let context = collect_git_context();
            assert!(context.is_some());

            let ctx = context.unwrap();
            assert_eq!(ctx.commit, Some("abc123def456".to_string()));
            // Branch should be None for detached HEAD
            assert_eq!(ctx.branch, None);
            assert_eq!(ctx.tag, None);
            assert_eq!(ctx.dirty, Some(false));
        });
    }

    #[test]
    #[serial]
    fn collect_git_context_handles_missing_commit() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");

        // Create a fake git that fails to get commit
        #[cfg(windows)]
        {
            let path = bin.join("git.cmd");
            fs::write(
                &path,
                "@echo off\r\nif not \"%1\"==\"rev-parse\" goto :not_revparse\r\nif \"%2\"==\"--git-dir\" exit /b 0\r\nif \"%2\"==\"HEAD\" exit /b 1\r\nif \"%2\"==\"--abbrev-ref\" echo main\r\nexit /b 0\r\n:not_revparse\r\nif \"%1\"==\"describe\" exit /b 1\r\nif \"%1\"==\"status\" exit /b 0\r\nexit /b 0\r\n",
            )
            .expect("write fake git");
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin.join("git");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nif [ \"$1\" = \"rev-parse\" ]; then\n  if [ \"$2\" = \"--git-dir\" ]; then\n    exit 0\n  fi\n  if [ \"$2\" = \"HEAD\" ]; then\n    exit 1\n  fi\n  if [ \"$2\" = \"--abbrev-ref\" ]; then\n    echo \"main\"\n    exit 0\n  fi\nfi\nif [ \"$1\" = \"describe\" ]; then\n  exit 1\nfi\nif [ \"$1\" = \"status\" ]; then\n  exit 0\nfi\n",
            )
            .expect("write fake git");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }

        let git_bin_path = bin
            .join(if cfg!(windows) { "git.cmd" } else { "git" })
            .to_str()
            .expect("utf8")
            .to_string();

        temp_env::with_var("SHIPPER_GIT_BIN", Some(&git_bin_path), || {
            let context = collect_git_context();
            assert!(context.is_some());

            let ctx = context.unwrap();
            // Commit should be None when git rev-parse fails
            assert_eq!(ctx.commit, None);
            assert_eq!(ctx.branch, Some("main".to_string()));
            assert_eq!(ctx.tag, None);
            assert_eq!(ctx.dirty, Some(false));
        });
    }
}
