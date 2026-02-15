use std::env;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct CargoOutput {
    pub exit_code: i32,
    pub stdout_tail: String, // Last N lines (configurable, default 50)
    pub stderr_tail: String,
    pub duration: Duration,
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        s.to_string()
    } else {
        lines[lines.len() - n..].join("\n")
    }
}

pub fn cargo_publish(
    workspace_root: &Path,
    package_name: &str,
    registry_name: &str,
    allow_dirty: bool,
    no_verify: bool,
    output_lines: usize,
    timeout: Option<Duration>,
) -> Result<CargoOutput> {
    let start = Instant::now();
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

    cmd.current_dir(workspace_root);

    let (exit_code, stdout, stderr) = if let Some(timeout_dur) = timeout {
        // Use spawn + polling to enforce timeout
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to execute cargo publish; is Cargo installed?")?;

        let deadline = Instant::now() + timeout_dur;
        loop {
            match child.try_wait().context("failed to poll cargo process")? {
                Some(status) => {
                    let out = child.wait_with_output().context("failed to read cargo output")?;
                    break (
                        status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&out.stdout).to_string(),
                        String::from_utf8_lossy(&out.stderr).to_string(),
                    );
                }
                None => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        anyhow::bail!(
                            "cargo publish timed out after {}",
                            humantime::format_duration(timeout_dur)
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    } else {
        let out = cmd
            .output()
            .context("failed to execute cargo publish; is Cargo installed?")?;
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };

    let duration = start.elapsed();

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
    })
}

pub fn cargo_publish_dry_run_workspace(
    workspace_root: &Path,
    registry_name: &str,
    allow_dirty: bool,
    output_lines: usize,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let mut cmd = Command::new(cargo_program());
    cmd.arg("publish").arg("--workspace").arg("--dry-run");

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        cmd.arg("--registry").arg(registry_name);
    }

    if allow_dirty {
        cmd.arg("--allow-dirty");
    }

    let out = cmd
        .current_dir(workspace_root)
        .output()
        .context("failed to execute cargo publish --dry-run --workspace; is Cargo installed?")?;

    let duration = start.elapsed();
    let exit_code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
    })
}

pub fn cargo_publish_dry_run_package(
    workspace_root: &Path,
    package_name: &str,
    registry_name: &str,
    allow_dirty: bool,
    output_lines: usize,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let mut cmd = Command::new(cargo_program());
    cmd.arg("publish")
        .arg("-p")
        .arg(package_name)
        .arg("--dry-run");

    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        cmd.arg("--registry").arg(registry_name);
    }

    if allow_dirty {
        cmd.arg("--allow-dirty");
    }

    let out = cmd.current_dir(workspace_root).output().with_context(|| {
        format!("failed to execute cargo publish --dry-run -p {package_name}; is Cargo installed?")
    })?;

    let duration = start.elapsed();
    let exit_code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
    })
}

fn cargo_program() -> String {
    env::var("SHIPPER_CARGO_BIN").unwrap_or_else(|_| "cargo".to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

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

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("7")),
            ],
            || {
                let out =
                    cargo_publish(&ws, "my-crate", "private-reg", true, true, 50, None)
                        .expect("publish");

                assert_eq!(out.exit_code, 7);
                assert!(out.stdout_tail.contains("fake-stdout"));
                assert!(out.stderr_tail.contains("fake-stderr"));

                let args = fs::read_to_string(&args_log).expect("args");
                assert!(args.contains("publish"));
                assert!(args.contains("-p my-crate"));
                assert!(args.contains("--registry private-reg"));
                assert!(args.contains("--allow-dirty"));
                assert!(args.contains("--no-verify"));

                let cwd = fs::read_to_string(&cwd_log).expect("cwd");
                assert!(cwd.trim_end().ends_with("workspace"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_omits_registry_for_crates_io() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ =
                    cargo_publish(&ws, "my-crate", "crates-io", false, false, 50, None)
                        .expect("publish");

                let args = fs::read_to_string(&args_log).expect("args");
                assert!(!args.contains("--registry"));
                assert!(!args.contains("--allow-dirty"));
                assert!(!args.contains("--no-verify"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_errors_when_command_missing() {
        let td = tempdir().expect("tempdir");
        let missing = td.path().join("does-not-exist-cargo");

        temp_env::with_var(
            "SHIPPER_CARGO_BIN",
            Some(missing.to_str().expect("utf8")),
            || {
                let err = cargo_publish(td.path(), "x", "crates-io", false, false, 50, None)
                    .expect_err("must fail");
                assert!(format!("{err:#}").contains("failed to execute cargo publish"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_package_passes_flags() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let out = cargo_publish_dry_run_package(&ws, "my-crate", "private-reg", true, 50)
                    .expect("dry-run");

                assert_eq!(out.exit_code, 0);
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(args.contains("publish"));
                assert!(args.contains("-p my-crate"));
                assert!(args.contains("--dry-run"));
                assert!(args.contains("--registry private-reg"));
                assert!(args.contains("--allow-dirty"));
            },
        );
    }
}
