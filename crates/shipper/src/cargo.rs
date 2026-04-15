use std::env;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use shipper_output_sanitizer::tail_lines as sanitize_tail_lines;

#[cfg(test)]
use shipper_output_sanitizer::redact_sensitive as sanitize_sensitive;

use crate::ops::process;

#[derive(Debug, Clone)]
pub struct CargoOutput {
    pub exit_code: i32,
    pub stdout_tail: String, // Last N lines (configurable, default 50)
    pub stderr_tail: String,
    pub duration: Duration,
    pub timed_out: bool,
}

fn tail_lines(s: &str, n: usize) -> String {
    sanitize_tail_lines(s, n)
}

/// Redact sensitive patterns (tokens, credentials) from output strings.
/// Applied to stdout/stderr tails before they are stored in receipts and event logs.
#[cfg(test)]
pub(crate) fn redact_sensitive(s: &str) -> String {
    sanitize_sensitive(s)
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

    #[test]
    fn redaction_matches_output_sanitizer_contract() {
        let input = [
            "line one",
            "Authorization: Bearer secret_value",
            "CARGO_REGISTRIES_PRIVATE_REG_TOKEN=secret_value",
        ]
        .join("\n");

        assert_eq!(
            redact_sensitive(&input),
            shipper_output_sanitizer::redact_sensitive(&input)
        );
        assert_eq!(
            tail_lines(&input, 2),
            shipper_output_sanitizer::tail_lines(&input, 2)
        );
    }

    // ── Token redaction: position variants ──

    #[test]
    fn redact_token_at_start_of_output() {
        let input = "CARGO_REGISTRY_TOKEN=start_secret\nnormal line after";
        let out = redact_sensitive(input);
        assert!(out.starts_with("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(!out.contains("start_secret"));
    }

    #[test]
    fn redact_token_at_end_of_output() {
        let input = "normal line\nCARGO_REGISTRY_TOKEN=end_secret";
        let out = redact_sensitive(input);
        assert!(out.ends_with("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(!out.contains("end_secret"));
    }

    #[test]
    fn redact_bearer_at_start_of_output() {
        let input = "Authorization: Bearer first_tok\nother stuff";
        let out = redact_sensitive(input);
        assert!(out.starts_with("Authorization: Bearer [REDACTED]"));
        assert!(!out.contains("first_tok"));
    }

    #[test]
    fn redact_bearer_at_end_of_output() {
        let input = "stuff before\nAuthorization: Bearer last_tok";
        let out = redact_sensitive(input);
        assert!(out.ends_with("Authorization: Bearer [REDACTED]"));
        assert!(!out.contains("last_tok"));
    }

    #[test]
    fn redact_token_as_only_line() {
        let out = redact_sensitive("CARGO_REGISTRY_TOKEN=only");
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
    }

    // ── Multiple tokens in same output ──

    #[test]
    fn redact_three_different_token_types_multiline() {
        let input = "Authorization: Bearer bearer_secret\n\
                      CARGO_REGISTRY_TOKEN=env_secret\n\
                      CARGO_REGISTRIES_STAGING_TOKEN=staging_secret";
        let out = redact_sensitive(input);
        assert!(!out.contains("bearer_secret"));
        assert!(!out.contains("env_secret"));
        assert!(!out.contains("staging_secret"));
        assert_eq!(out.matches("[REDACTED]").count(), 3);
    }

    #[test]
    fn redact_same_token_type_repeated() {
        let input = "CARGO_REGISTRY_TOKEN=aaa\nsome stuff\nCARGO_REGISTRY_TOKEN=bbb";
        let out = redact_sensitive(input);
        assert!(!out.contains("aaa"));
        assert!(!out.contains("bbb"));
        assert_eq!(
            out,
            "CARGO_REGISTRY_TOKEN=[REDACTED]\nsome stuff\nCARGO_REGISTRY_TOKEN=[REDACTED]"
        );
    }

    #[test]
    fn redact_multiple_named_registries() {
        let input = "CARGO_REGISTRIES_ALPHA_TOKEN=tok_a\n\
                      CARGO_REGISTRIES_BETA_TOKEN=tok_b\n\
                      CARGO_REGISTRIES_GAMMA_TOKEN=tok_c";
        let out = redact_sensitive(input);
        assert!(!out.contains("tok_a"));
        assert!(!out.contains("tok_b"));
        assert!(!out.contains("tok_c"));
        assert_eq!(out.matches("[REDACTED]").count(), 3);
    }

    // ── Unicode in cargo output ──

    #[test]
    fn redact_preserves_cjk_characters() {
        let input = "コンパイル中: mycrate v1.0.0\n完了";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_preserves_emoji_in_output() {
        let input = "🚀 Publishing crate 📦\n✅ Done!";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_unicode_surrounding_bearer_token() {
        let input = "日本語テスト Authorization: Bearer abc_secret 中文テスト";
        let out = redact_sensitive(input);
        assert!(!out.contains("abc_secret"));
        assert!(out.contains("日本語テスト"));
        // Bearer redaction truncates after token, so 中文テスト is part of the token value
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn redact_accented_characters_preserved() {
        let input = "Résultat: réussi\nDéploiement terminé";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn tail_lines_with_unicode_content() {
        let input = "first 日本語\nsecond émoji 🎉\nthird 中文";
        let out = tail_lines(input, 2);
        assert_eq!(out, "second émoji 🎉\nthird 中文");
    }

    // ── Very long output lines ──

    #[test]
    fn redact_very_long_line_no_token() {
        let long_line = "x".repeat(500_000);
        let out = redact_sensitive(&long_line);
        assert_eq!(out.len(), 500_000);
        assert_eq!(out, long_line);
    }

    #[test]
    fn redact_token_embedded_in_very_long_line() {
        let prefix = "a".repeat(200_000);
        let suffix = "b".repeat(200_000);
        let input = format!("{prefix} CARGO_REGISTRY_TOKEN=hidden {suffix}");
        let out = redact_sensitive(&input);
        assert!(!out.contains("hidden"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn tail_lines_with_very_long_lines() {
        let long = "y".repeat(100_000);
        let input = format!("short\n{long}\nlast");
        let out = tail_lines(&input, 2);
        assert!(out.contains(&long));
        assert!(out.contains("last"));
        assert!(!out.contains("short"));
    }

    // ── Empty output handling ──

    #[test]
    fn tail_lines_empty_string() {
        assert_eq!(tail_lines("", 10), "");
    }

    #[test]
    fn tail_lines_only_newlines() {
        let input = "\n\n\n";
        let out = tail_lines(input, 2);
        // .lines() yields three empty strings for "\n\n\n"
        assert!(out.lines().all(|l| l.is_empty()));
    }

    #[test]
    fn tail_lines_single_newline() {
        let out = tail_lines("\n", 5);
        // "\n".lines() yields one empty string
        assert_eq!(out, "\n");
    }

    #[test]
    fn redact_whitespace_only_input() {
        let input = "   \t  ";
        assert_eq!(redact_sensitive(input), input);
    }

    #[test]
    fn tail_lines_whitespace_only_lines() {
        let input = "  \n\t\n   ";
        let out = tail_lines(input, 2);
        assert_eq!(out, "\t\n   ");
    }

    // ── Timeout behavior ──

    #[test]
    #[serial]
    fn cargo_publish_with_timeout_captures_timed_out_flag() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");

        // Write a fake cargo that sleeps, ensuring it exceeds the timeout
        #[cfg(windows)]
        {
            let path = bin.join("cargo.cmd");
            fs::write(
                &path,
                "@echo off\r\nping -n 5 127.0.0.1 >nul\r\necho should-not-see\r\n",
            )
            .expect("write slow fake cargo");
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin.join("cargo");
            fs::write(&path, "#!/usr/bin/env sh\nsleep 10\necho should-not-see\n")
                .expect("write slow fake cargo");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }

        let fake_cargo_path = if cfg!(windows) {
            bin.join("cargo.cmd")
        } else {
            bin.join("cargo")
        };

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [(
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path.to_str().expect("utf8")),
            )],
            || {
                let out = cargo_publish(
                    &ws,
                    "test-crate",
                    "crates-io",
                    false,
                    false,
                    50,
                    Some(Duration::from_secs(1)),
                )
                .expect("publish with timeout");

                assert!(out.timed_out, "expected timed_out flag to be set");
                assert_eq!(out.exit_code, -1);
                assert!(out.stderr_tail.contains("timed out"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_no_timeout_completes_normally() {
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
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let out = cargo_publish(&ws, "crate-x", "crates-io", false, false, 50, None)
                    .expect("publish");
                assert!(!out.timed_out, "should not time out");
                assert_eq!(out.exit_code, 0);
            },
        );
    }

    // ── Environment variable resolution / cargo_program ──

    #[test]
    #[serial]
    fn cargo_program_uses_env_override() {
        temp_env::with_var("SHIPPER_CARGO_BIN", Some("/custom/cargo"), || {
            assert_eq!(cargo_program(), "/custom/cargo");
        });
    }

    #[test]
    #[serial]
    fn cargo_program_defaults_to_cargo() {
        temp_env::with_var("SHIPPER_CARGO_BIN", None::<&str>, || {
            assert_eq!(cargo_program(), "cargo");
        });
    }

    #[test]
    #[serial]
    fn cargo_program_with_empty_env_uses_empty_string() {
        // Empty string is a valid env value; cargo_program returns it as-is
        temp_env::with_var("SHIPPER_CARGO_BIN", Some(""), || {
            assert_eq!(cargo_program(), "");
        });
    }

    // ── Registry name handling ──

    #[test]
    #[serial]
    fn cargo_publish_omits_registry_for_empty_string() {
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
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ = cargo_publish(&ws, "crate-y", "", false, false, 50, None).expect("publish");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(
                    !args.contains("--registry"),
                    "empty registry name should not produce --registry flag"
                );
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_omits_registry_for_whitespace_only() {
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
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ =
                    cargo_publish(&ws, "crate-z", "   ", false, false, 50, None).expect("publish");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(
                    !args.contains("--registry"),
                    "whitespace-only registry name should not produce --registry flag"
                );
            },
        );
    }

    // ── Dry-run workspace variant ──

    #[test]
    #[serial]
    fn cargo_publish_dry_run_workspace_passes_flags() {
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
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let out =
                    cargo_publish_dry_run_workspace(&ws, "my-reg", true, 50).expect("dry-run ws");

                assert_eq!(out.exit_code, 0);
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(args.contains("publish"));
                assert!(args.contains("--workspace"));
                assert!(args.contains("--dry-run"));
                assert!(args.contains("--registry my-reg"));
                assert!(args.contains("--allow-dirty"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_workspace_omits_registry_for_crates_io() {
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
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ =
                    cargo_publish_dry_run_workspace(&ws, "crates-io", false, 50).expect("dry-run");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(!args.contains("--registry"));
                assert!(!args.contains("--allow-dirty"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_workspace_errors_when_command_missing() {
        let td = tempdir().expect("tempdir");
        let missing = td.path().join("nonexistent-cargo");

        temp_env::with_var(
            "SHIPPER_CARGO_BIN",
            Some(missing.to_str().expect("utf8")),
            || {
                let err = cargo_publish_dry_run_workspace(td.path(), "crates-io", false, 50)
                    .expect_err("must fail");
                assert!(format!("{err:#}").contains("failed to execute cargo publish"));
            },
        );
    }

    // ── Dry-run package variant additional tests ──

    #[test]
    #[serial]
    fn cargo_publish_dry_run_package_omits_registry_for_crates_io() {
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
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ = cargo_publish_dry_run_package(&ws, "pkg", "crates-io", false, 50)
                    .expect("dry-run");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(!args.contains("--registry"));
                assert!(!args.contains("--allow-dirty"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_package_errors_when_command_missing() {
        let td = tempdir().expect("tempdir");
        let missing = td.path().join("nonexistent-cargo");

        temp_env::with_var(
            "SHIPPER_CARGO_BIN",
            Some(missing.to_str().expect("utf8")),
            || {
                let err = cargo_publish_dry_run_package(td.path(), "pkg", "crates-io", false, 50)
                    .expect_err("must fail");
                let msg = format!("{err:#}");
                assert!(msg.contains("failed to execute cargo publish --dry-run -p pkg"));
            },
        );
    }

    // ── Output line truncation via tail_lines ──

    #[test]
    fn tail_lines_truncates_to_requested_count() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        let input = lines.join("\n");
        let out = tail_lines(&input, 5);
        assert_eq!(out.lines().count(), 5);
        assert!(out.contains("line 95"));
        assert!(out.contains("line 99"));
        assert!(!out.contains("line 94"));
    }

    #[test]
    fn tail_lines_one_line_requested() {
        let input = "first\nsecond\nthird";
        let out = tail_lines(input, 1);
        assert_eq!(out, "third");
    }

    #[test]
    fn tail_lines_redacts_token_in_last_line() {
        let input = "safe1\nsafe2\nCARGO_REGISTRY_TOKEN=leaked";
        let out = tail_lines(input, 2);
        assert!(!out.contains("leaked"));
        assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
    }

    #[test]
    fn tail_lines_token_outside_window_not_visible() {
        let input = "CARGO_REGISTRY_TOKEN=secret\nsafe1\nsafe2";
        let out = tail_lines(input, 2);
        assert!(!out.contains("secret"));
        assert!(!out.contains("CARGO_REGISTRY_TOKEN"));
        assert_eq!(out, "safe1\nsafe2");
    }

    // ── Error message patterns ──

    #[test]
    fn redact_token_in_error_message_context() {
        let input =
            "error: failed to publish: token = \"cio_leakedsecret\" was rejected by registry";
        let out = redact_sensitive(input);
        assert!(!out.contains("cio_leakedsecret"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn redact_bearer_in_http_error() {
        let input =
            "error: HTTP 403 Forbidden\nAuthorization: Bearer expired_tok_abc\nBody: access denied";
        let out = redact_sensitive(input);
        assert!(!out.contains("expired_tok_abc"));
        assert!(out.contains("error: HTTP 403 Forbidden"));
        assert!(out.contains("Body: access denied"));
    }

    #[test]
    fn redact_registry_token_in_debug_output() {
        let input = "debug: env CARGO_REGISTRY_TOKEN=cio_debug_tok resolved from environment";
        let out = redact_sensitive(input);
        assert!(!out.contains("cio_debug_tok"));
        assert!(out.contains("[REDACTED]"));
    }

    // ── CargoOutput struct behavior ──

    #[test]
    fn cargo_output_default_fields() {
        let out = CargoOutput {
            exit_code: 0,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            duration: Duration::from_secs(0),
            timed_out: false,
        };
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout_tail.is_empty());
        assert!(out.stderr_tail.is_empty());
        assert!(!out.timed_out);
    }

    #[test]
    fn cargo_output_clone_is_independent() {
        let out = CargoOutput {
            exit_code: 42,
            stdout_tail: "hello".to_string(),
            stderr_tail: "world".to_string(),
            duration: Duration::from_millis(500),
            timed_out: true,
        };
        let cloned = out.clone();
        assert_eq!(cloned.exit_code, out.exit_code);
        assert_eq!(cloned.stdout_tail, out.stdout_tail);
        assert_eq!(cloned.stderr_tail, out.stderr_tail);
        assert_eq!(cloned.timed_out, out.timed_out);
    }

    #[test]
    fn cargo_output_debug_format() {
        let out = CargoOutput {
            exit_code: 1,
            stdout_tail: "out".to_string(),
            stderr_tail: "err".to_string(),
            duration: Duration::from_secs(1),
            timed_out: false,
        };
        let debug = format!("{out:?}");
        assert!(debug.contains("CargoOutput"));
        assert!(debug.contains("exit_code: 1"));
    }

    // ── Redaction idempotency ──

    #[test]
    fn redact_is_idempotent_bearer() {
        let input = "Authorization: Bearer secret_value";
        let once = redact_sensitive(input);
        let twice = redact_sensitive(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn redact_is_idempotent_env_token() {
        let input = "CARGO_REGISTRY_TOKEN=secret";
        let once = redact_sensitive(input);
        let twice = redact_sensitive(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn redact_is_idempotent_token_assignment() {
        let input = r#"token = "secret_value""#;
        let once = redact_sensitive(input);
        let twice = redact_sensitive(&once);
        assert_eq!(once, twice);
    }

    // ── Non-default exit codes ──

    #[test]
    #[serial]
    fn cargo_publish_captures_nonzero_exit_code() {
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
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("101")),
            ],
            || {
                let out = cargo_publish(&ws, "crate-a", "crates-io", false, false, 50, None)
                    .expect("publish");
                assert_eq!(out.exit_code, 101);
                assert!(!out.timed_out);
            },
        );
    }

    // ── tail_lines with output_lines = 0 (edge case for output truncation) ──

    #[test]
    fn tail_lines_zero_returns_empty() {
        let input = "line1\nline2\nline3";
        assert_eq!(tail_lines(input, 0), "");
    }

    // ── Redaction with special characters in token values ──

    #[test]
    fn redact_token_with_special_chars() {
        let input = "CARGO_REGISTRY_TOKEN=abc!@#$%^&*()_+-=[]{}|;:',.<>?/";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_bearer_with_base64_padding() {
        let input = "Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.payload.sig==";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_token_value_with_newline_escapes() {
        // Token value should not contain literal newlines, but escaped ones may appear
        let input = r#"token = "secret\nwith\nescapes""#;
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("secret\\nwith"));
    }
}
