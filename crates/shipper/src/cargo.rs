use std::env;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::process;

#[derive(Debug, Clone)]
pub struct CargoOutput {
    pub exit_code: i32,
    pub stdout_tail: String, // Last N lines (configurable, default 50)
    pub stderr_tail: String,
    pub duration: Duration,
    pub timed_out: bool,
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let tail = if lines.len() <= n {
        s.to_string()
    } else {
        lines[lines.len() - n..].join("\n")
    };
    redact_sensitive(&tail)
}

/// Redact sensitive patterns (tokens, credentials) from output strings.
/// Applied to stdout/stderr tails before they are stored in receipts and event logs.
pub(crate) fn redact_sensitive(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for line in s.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&redact_line(line));
    }
    // Preserve trailing newline if present
    if s.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn redact_line(line: &str) -> String {
    let mut out = line.to_string();

    // Authorization: Bearer <token>
    if let Some(pos) = out.to_ascii_lowercase().find("authorization:") {
        let after = &out[pos..];
        if let Some(bearer_pos) = after.to_ascii_lowercase().find("bearer ") {
            let redact_start = pos + bearer_pos + "bearer ".len();
            out = format!("{}[REDACTED]", &out[..redact_start]);
        }
    }

    // token = "<value>" or token = '<value>' or token = <value>
    if let Some(pos) = out.to_ascii_lowercase().find("token") {
        let after_key = &out[pos + "token".len()..];
        let trimmed = after_key.trim_start();
        if trimmed.starts_with("= ") || trimmed.starts_with("=") {
            let eq_offset = pos + "token".len() + (after_key.len() - trimmed.len());
            let after_eq = trimmed.trim_start_matches('=').trim_start();
            // Check for quoted or unquoted value
            if after_eq.starts_with('"') || after_eq.starts_with('\'') {
                out = format!("{}= \"[REDACTED]\"", &out[..eq_offset]);
            } else if !after_eq.is_empty() {
                out = format!("{}= [REDACTED]", &out[..eq_offset]);
            }
        }
    }

    // CARGO_REGISTRY_TOKEN=<value> and CARGO_REGISTRIES_<NAME>_TOKEN=<value>
    if let Some(pos) = find_cargo_token_env(&out)
        && let Some(eq_pos) = out[pos..].find('=')
    {
        let abs_eq = pos + eq_pos;
        out = format!("{}=[REDACTED]", &out[..abs_eq]);
    }

    out
}

/// Find the start position of a CARGO_REGISTRY_TOKEN or CARGO_REGISTRIES_<NAME>_TOKEN pattern.
fn find_cargo_token_env(s: &str) -> Option<usize> {
    // Check for CARGO_REGISTRY_TOKEN
    if let Some(pos) = s.find("CARGO_REGISTRY_TOKEN") {
        return Some(pos);
    }
    // Check for CARGO_REGISTRIES_<NAME>_TOKEN
    if let Some(pos) = s.find("CARGO_REGISTRIES_") {
        let after = &s[pos + "CARGO_REGISTRIES_".len()..];
        if after.contains("_TOKEN") {
            return Some(pos);
        }
    }
    None
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
    let mut args: Vec<&str> = Vec::new();
    args.push("publish");
    args.push("-p");
    args.push(package_name);

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    if allow_dirty {
        args.push("--allow-dirty");
    }
    if no_verify {
        args.push("--no-verify");
    }

    let output =
        process::run_command_with_timeout(&cargo_program(), &args, workspace_root, timeout)
            .context("failed to execute cargo publish; is Cargo installed?")?;

    let exit_code = output.exit_code;
    let stdout = output.stdout;
    let stderr = output.stderr;
    let timed_out = output.timed_out;

    let duration = start.elapsed();

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out,
    })
}

pub fn cargo_publish_dry_run_workspace(
    workspace_root: &Path,
    registry_name: &str,
    allow_dirty: bool,
    output_lines: usize,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let mut args: Vec<&str> = vec!["publish", "--workspace", "--dry-run"];

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    if allow_dirty {
        args.push("--allow-dirty");
    }

    let output = process::run_command_with_timeout(&cargo_program(), &args, workspace_root, None)
        .context(
        "failed to execute cargo publish --dry-run --workspace; is Cargo installed?",
    )?;

    let duration = start.elapsed();
    let exit_code = output.exit_code;
    let stdout = output.stdout;
    let stderr = output.stderr;
    let timed_out = output.timed_out;

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out,
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
    let mut args: Vec<&str> = vec!["publish", "-p", package_name, "--dry-run"];

    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    if allow_dirty {
        args.push("--allow-dirty");
    }

    let output = process::run_command_with_timeout(&cargo_program(), &args, workspace_root, None)
        .with_context(|| {
        format!("failed to execute cargo publish --dry-run -p {package_name}; is Cargo installed?")
    })?;

    let duration = start.elapsed();
    let exit_code = output.exit_code;
    let stdout = output.stdout;
    let stderr = output.stderr;
    let timed_out = output.timed_out;

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out,
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
                let out = cargo_publish(&ws, "my-crate", "private-reg", true, true, 50, None)
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
                let _ = cargo_publish(&ws, "my-crate", "crates-io", false, false, 50, None)
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

    // ── redact_sensitive tests ──

    #[test]
    fn redact_authorization_bearer_header() {
        let input = "Authorization: Bearer cio_abc123secret";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_token_assignment_quoted() {
        let input = r#"token = "cio_mysecrettoken""#;
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("cio_mysecrettoken"));
    }

    #[test]
    fn redact_cargo_registry_token_env() {
        let input = "CARGO_REGISTRY_TOKEN=cio_secret123";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_cargo_registries_named_token_env() {
        let input = "CARGO_REGISTRIES_MY_REG_TOKEN=secret456";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRIES_MY_REG_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_preserves_non_sensitive_content() {
        let input = "Compiling demo v0.1.0\nFinished release target";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_handles_empty_input() {
        assert_eq!(redact_sensitive(""), "");
    }

    #[test]
    fn redact_multiple_sensitive_patterns() {
        let input = "Authorization: Bearer tok123\nCARGO_REGISTRY_TOKEN=secret";
        let out = redact_sensitive(input);
        assert!(out.contains("Bearer [REDACTED]"));
        assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(!out.contains("tok123"));
        assert!(!out.contains("secret"));
    }

    #[test]
    fn tail_lines_redacts_sensitive_output() {
        let input = "line1\nline2\nAuthorization: Bearer secret_token\nline4";
        let result = tail_lines(input, 50);
        assert!(result.contains("Bearer [REDACTED]"));
        assert!(!result.contains("secret_token"));
    }

    #[test]
    fn redact_mixed_case_authorization() {
        let input = "AUTHORIZATION: Bearer supersecret";
        let out = redact_sensitive(input);
        assert_eq!(out, "AUTHORIZATION: Bearer [REDACTED]");
        assert!(!out.contains("supersecret"));
    }

    #[test]
    fn redact_mixed_case_token() {
        let input = r#"Token = "mysecret""#;
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("mysecret"));
    }

    #[test]
    fn redact_non_ascii_near_sensitive_pattern_no_panic() {
        // Non-ASCII characters near the pattern should not cause a panic
        let input = "some data \u{00e9}\u{00f1} Authorization: Bearer secret123";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("secret123"));
    }
}
