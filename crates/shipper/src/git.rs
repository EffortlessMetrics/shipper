use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

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
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

    struct EnvGuard {
        key: String,
        old: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let old = env::var(key).ok();
            unsafe { env::set_var(key, value) };
            Self {
                key: key.to_string(),
                old,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = &self.old {
                unsafe { env::set_var(&self.key, v) };
            } else {
                unsafe { env::remove_var(&self.key) };
            }
        }
    }

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

    #[test]
    #[serial]
    fn is_git_clean_true_when_porcelain_empty() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);
        let _program = EnvGuard::set("SHIPPER_GIT_BIN", fake_git.to_str().expect("utf8"));
        let _mode = EnvGuard::set("SHIPPER_GIT_MODE", "clean");

        let ok = is_git_clean(td.path()).expect("git clean");
        assert!(ok);
    }

    #[test]
    #[serial]
    fn is_git_clean_false_when_porcelain_has_changes() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);
        let _program = EnvGuard::set("SHIPPER_GIT_BIN", fake_git.to_str().expect("utf8"));
        let _mode = EnvGuard::set("SHIPPER_GIT_MODE", "dirty");

        let ok = is_git_clean(td.path()).expect("git clean");
        assert!(!ok);
    }

    #[test]
    #[serial]
    fn is_git_clean_surfaces_git_failure() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);
        let _program = EnvGuard::set("SHIPPER_GIT_BIN", fake_git.to_str().expect("utf8"));
        let _mode = EnvGuard::set("SHIPPER_GIT_MODE", "fail");

        let err = is_git_clean(td.path()).expect_err("must fail");
        assert!(format!("{err:#}").contains("git status failed"));
    }

    #[test]
    #[serial]
    fn ensure_git_clean_errors_for_dirty_tree() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_git = write_fake_git(&bin);
        let _program = EnvGuard::set("SHIPPER_GIT_BIN", fake_git.to_str().expect("utf8"));
        let _mode = EnvGuard::set("SHIPPER_GIT_MODE", "dirty");

        let err = ensure_git_clean(td.path()).expect_err("must fail");
        assert!(format!("{err:#}").contains("git working tree is not clean"));
    }

    #[test]
    #[serial]
    fn env_guard_restores_existing_value() {
        unsafe { env::set_var("SHIPPER_GIT_TMP_TEST", "old") };
        {
            let _guard = EnvGuard::set("SHIPPER_GIT_TMP_TEST", "new");
            assert_eq!(
                env::var("SHIPPER_GIT_TMP_TEST").expect("present"),
                "new".to_string()
            );
        }
        assert_eq!(
            env::var("SHIPPER_GIT_TMP_TEST").expect("present"),
            "old".to_string()
        );
        unsafe { env::remove_var("SHIPPER_GIT_TMP_TEST") };
    }
}
