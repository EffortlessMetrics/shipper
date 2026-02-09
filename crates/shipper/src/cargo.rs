use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct CargoOutput {
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn cargo_publish(
    workspace_root: &Path,
    package_name: &str,
    registry_name: &str,
    allow_dirty: bool,
    no_verify: bool,
) -> Result<CargoOutput> {
    let mut cmd = Command::new(cargo_program());
    cmd.arg("publish").arg("-p").arg(package_name);

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        cmd.arg("--registry").arg(registry_name);
    }

    if allow_dirty {
        cmd.arg("--allow-dirty");
    }
    if no_verify {
        cmd.arg("--no-verify");
    }

    let out = cmd
        .current_dir(workspace_root)
        .output()
        .context("failed to execute cargo publish; is Cargo installed?")?;

    Ok(CargoOutput {
        status_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

fn cargo_program() -> String {
    env::var("SHIPPER_CARGO_BIN").unwrap_or_else(|_| "cargo".to_string())
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

    fn write_fake_cargo(bin_dir: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let path = bin_dir.join("cargo.cmd");
            fs::write(
                &path,
                "@echo off\r\necho %*>\"%SHIPPER_ARGS_LOG%\"\r\necho %CD%>\"%SHIPPER_CWD_LOG%\"\r\necho fake-stdout\r\necho fake-stderr 1>&2\r\nexit /b %SHIPPER_EXIT_CODE%\r\n",
            )
            .expect("write fake cargo");
            path
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("cargo");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nprintf '%s' \"$*\" >\"$SHIPPER_ARGS_LOG\"\npwd >\"$SHIPPER_CWD_LOG\"\necho fake-stdout\necho fake-stderr >&2\nexit \"${SHIPPER_EXIT_CODE:-0}\"\n",
            )
            .expect("write fake cargo");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
            path
        }
    }

    #[test]
    #[serial]
    fn cargo_publish_passes_flags_and_captures_output() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);
        let _program = EnvGuard::set(
            "SHIPPER_CARGO_BIN",
            fake_cargo.to_str().expect("fake cargo utf8"),
        );

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");
        let _a = EnvGuard::set("SHIPPER_ARGS_LOG", args_log.to_str().expect("utf8"));
        let _b = EnvGuard::set("SHIPPER_CWD_LOG", cwd_log.to_str().expect("utf8"));
        let _c = EnvGuard::set("SHIPPER_EXIT_CODE", "7");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        let out = cargo_publish(&ws, "my-crate", "private-reg", true, true).expect("publish");

        assert_eq!(out.status_code, Some(7));
        assert!(out.stdout.contains("fake-stdout"));
        assert!(out.stderr.contains("fake-stderr"));

        let args = fs::read_to_string(args_log).expect("args");
        assert!(args.contains("publish"));
        assert!(args.contains("-p my-crate"));
        assert!(args.contains("--registry private-reg"));
        assert!(args.contains("--allow-dirty"));
        assert!(args.contains("--no-verify"));

        let cwd = fs::read_to_string(cwd_log).expect("cwd");
        assert!(cwd.trim_end().ends_with("workspace"));
    }

    #[test]
    #[serial]
    fn cargo_publish_omits_registry_for_crates_io() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);
        let _program = EnvGuard::set(
            "SHIPPER_CARGO_BIN",
            fake_cargo.to_str().expect("fake cargo utf8"),
        );

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");
        let _a = EnvGuard::set("SHIPPER_ARGS_LOG", args_log.to_str().expect("utf8"));
        let _b = EnvGuard::set("SHIPPER_CWD_LOG", cwd_log.to_str().expect("utf8"));
        let _c = EnvGuard::set("SHIPPER_EXIT_CODE", "0");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        let _ = cargo_publish(&ws, "my-crate", "crates-io", false, false).expect("publish");

        let args = fs::read_to_string(args_log).expect("args");
        assert!(!args.contains("--registry"));
        assert!(!args.contains("--allow-dirty"));
        assert!(!args.contains("--no-verify"));
    }

    #[test]
    #[serial]
    fn cargo_publish_errors_when_command_missing() {
        let td = tempdir().expect("tempdir");
        let missing = td.path().join("does-not-exist-cargo");
        let _program = EnvGuard::set("SHIPPER_CARGO_BIN", missing.to_str().expect("utf8"));

        let err = cargo_publish(td.path(), "x", "crates-io", false, false).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to execute cargo publish"));
    }

    #[test]
    #[serial]
    fn env_guard_restores_existing_value() {
        unsafe { env::set_var("SHIPPER_TMP_TEST_KEY", "old") };
        {
            let _guard = EnvGuard::set("SHIPPER_TMP_TEST_KEY", "new");
            assert_eq!(
                env::var("SHIPPER_TMP_TEST_KEY").expect("present"),
                "new".to_string()
            );
        }
        assert_eq!(
            env::var("SHIPPER_TMP_TEST_KEY").expect("present"),
            "old".to_string()
        );
        unsafe { env::remove_var("SHIPPER_TMP_TEST_KEY") };
    }
}
