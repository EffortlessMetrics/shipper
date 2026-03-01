//! Process execution for shipper.
//!
//! This crate provides utilities for running external processes
//! with proper error handling, timeouts, and output capture.
//!
//! # Example
//!
//! ```ignore
//! use shipper_process::{run_command, CommandResult};
//!
//! // Run a simple command
//! let result = run_command("cargo", &["--version"]).expect("run");
//! assert!(result.success);
//! assert!(result.stdout.contains("cargo"));
//! ```

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Result of a command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    /// Whether the command succeeded (exit code 0)
    pub success: bool,
    /// Exit code (if available)
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Duration of execution
    pub duration_ms: u64,
}

impl CommandResult {
    /// Check if the command succeeded
    pub fn ok(&self) -> Result<&Self> {
        if self.success {
            Ok(self)
        } else {
            Err(anyhow::anyhow!(
                "command failed with exit code {:?}: {}",
                self.exit_code,
                self.stderr
            ))
        }
    }

    /// Create a result from a process output
    pub fn from_output(output: &Output, duration: Duration) -> Self {
        Self {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            duration_ms: duration.as_millis() as u64,
        }
    }
}

/// Result of a command execution with timeout bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    /// Exit code (or -1 when not available)
    pub exit_code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Whether execution exceeded timeout.
    pub timed_out: bool,
    /// Total wall-clock duration.
    pub duration: Duration,
}

/// Run a command and capture its output
pub fn run_command(program: &str, args: &[&str]) -> Result<CommandResult> {
    let start = std::time::Instant::now();

    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run command: {} {:?}", program, args))?;

    Ok(CommandResult::from_output(&output, start.elapsed()))
}

/// Run a command in a specific directory
pub fn run_command_in_dir(
    program: &str,
    args: &[&str],
    dir: &std::path::Path,
) -> Result<CommandResult> {
    let start = std::time::Instant::now();

    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| {
            format!(
                "failed to run command: {} {:?} in {}",
                program,
                args,
                dir.display()
            )
        })?;

    Ok(CommandResult::from_output(&output, start.elapsed()))
}

/// Run a command with optional timeout and captured output.
pub fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    working_dir: &std::path::Path,
    timeout: Option<Duration>,
) -> Result<CommandOutput> {
    let start = Instant::now();

    let Some(timeout_dur) = timeout else {
        let output = run_command_in_dir(program, args, working_dir)?;
        return Ok(CommandOutput {
            exit_code: output.exit_code.unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: false,
            duration: Duration::from_millis(output.duration_ms),
        });
    };

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn command: {}", program))?;

    let deadline = Instant::now() + timeout_dur;
    loop {
        match child
            .try_wait()
            .with_context(|| format!("failed to poll command: {}", program))?
        {
            Some(status) => {
                return Ok(CommandOutput {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: read_pipe(child.stdout.take()),
                    stderr: read_pipe(child.stderr.take()),
                    timed_out: false,
                    duration: start.elapsed(),
                });
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();

                    let mut stderr = read_pipe(child.stderr.take());
                    stderr.push_str(&format!(
                        "\n{} timed out after {}",
                        program,
                        humantime::format_duration(timeout_dur)
                    ));

                    return Ok(CommandOutput {
                        exit_code: -1,
                        stdout: read_pipe(child.stdout.take()),
                        stderr,
                        timed_out: true,
                        duration: start.elapsed(),
                    });
                }

                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn read_pipe<R: Read>(stream: Option<R>) -> String {
    let mut buffer = Vec::new();
    if let Some(mut s) = stream {
        let _ = s.read_to_end(&mut buffer);
    }
    String::from_utf8_lossy(&buffer).to_string()
}

/// Run a command with environment variables
pub fn run_command_with_env(
    program: &str,
    args: &[&str],
    env: &[(String, String)],
) -> Result<CommandResult> {
    let start = std::time::Instant::now();

    let mut cmd = Command::new(program);
    cmd.args(args);

    for (key, value) in env {
        cmd.env(key, value);
    }

    let output = cmd
        .output()
        .with_context(|| format!("failed to run command: {} {:?}", program, args))?;

    Ok(CommandResult::from_output(&output, start.elapsed()))
}

/// Run a command and stream output to stdout/stderr
pub fn run_command_streaming(program: &str, args: &[&str]) -> Result<CommandResult> {
    let start = std::time::Instant::now();

    let output = Command::new(program)
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("failed to run command: {} {:?}", program, args))?;

    Ok(CommandResult::from_output(&output, start.elapsed()))
}

/// Run a command and return success/failure without capturing output
pub fn run_command_simple(program: &str, args: &[&str]) -> Result<bool> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run command: {} {:?}", program, args))?;

    Ok(status.success())
}

/// Check if a command exists in PATH
pub fn command_exists(program: &str) -> bool {
    which::which(program).is_ok()
}

/// Get the full path to a command
pub fn which(program: &str) -> Option<std::path::PathBuf> {
    which::which(program).ok()
}

/// Run cargo with arguments
pub fn run_cargo(args: &[&str]) -> Result<CommandResult> {
    run_command("cargo", args)
}

/// Run cargo in a specific directory
pub fn run_cargo_in_dir(args: &[&str], dir: &std::path::Path) -> Result<CommandResult> {
    run_command_in_dir("cargo", args, dir)
}

/// Run cargo publish (dry run)
pub fn cargo_dry_run(manifest_path: &std::path::Path) -> Result<CommandResult> {
    run_cargo_in_dir(
        &[
            "publish",
            "--dry-run",
            "--manifest-path",
            manifest_path.to_str().unwrap_or(""),
        ],
        manifest_path.parent().unwrap_or(std::path::Path::new(".")),
    )
}

/// Run cargo publish
pub fn cargo_publish(
    manifest_path: &std::path::Path,
    registry: Option<&str>,
) -> Result<CommandResult> {
    let mut args = vec![
        "publish",
        "--manifest-path",
        manifest_path.to_str().unwrap_or(""),
    ];

    if let Some(reg) = registry {
        args.push("--registry");
        args.push(reg);
    }

    run_cargo_in_dir(
        &args,
        manifest_path.parent().unwrap_or(std::path::Path::new(".")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_version() {
        let result = run_command("cargo", &["--version"]).expect("run");
        assert!(result.success);
        assert!(result.stdout.contains("cargo"));
    }

    #[test]
    fn run_command_failure() {
        let result = run_command("cargo", &["--nonexistent-flag-xyz"]).expect("run");
        assert!(!result.success);
    }

    #[test]
    fn command_result_ok() {
        let result = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: "output".to_string(),
            stderr: "".to_string(),
            duration_ms: 100,
        };

        assert!(result.ok().is_ok());
    }

    #[test]
    fn command_result_err() {
        let result = CommandResult {
            success: false,
            exit_code: Some(1),
            stdout: "".to_string(),
            stderr: "error".to_string(),
            duration_ms: 100,
        };

        assert!(result.ok().is_err());
    }

    #[test]
    fn run_command_simple_cargo() {
        let success = run_command_simple("cargo", &["--version"]).expect("run");
        assert!(success);
    }

    #[test]
    fn command_exists_cargo() {
        assert!(command_exists("cargo"));
    }

    #[test]
    fn command_exists_nonexistent() {
        assert!(!command_exists("this-command-does-not-exist-xyz123"));
    }

    #[test]
    fn which_cargo() {
        let path = which("cargo");
        assert!(path.is_some());
    }

    #[test]
    fn run_cargo_version() {
        let result = run_cargo(&["--version"]).expect("run");
        assert!(result.success);
        assert!(result.stdout.contains("cargo"));
    }

    #[test]
    fn command_result_serialization() {
        let result = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: "output".to_string(),
            stderr: "".to_string(),
            duration_ms: 150,
        };

        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"stdout\":\"output\""));
    }

    // ── CommandResult unit tests ──────────────────────────────────────

    #[test]
    fn command_result_ok_returns_self_ref() {
        let result = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: "hello".to_string(),
            stderr: String::new(),
            duration_ms: 10,
        };
        let r = result.ok().expect("should be ok");
        assert_eq!(r.stdout, "hello");
    }

    #[test]
    fn command_result_err_contains_exit_code_and_stderr() {
        let result = CommandResult {
            success: false,
            exit_code: Some(42),
            stdout: String::new(),
            stderr: "boom".to_string(),
            duration_ms: 5,
        };
        let err = result.ok().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("42"), "should mention exit code: {msg}");
        assert!(msg.contains("boom"), "should mention stderr: {msg}");
    }

    #[test]
    fn command_result_err_none_exit_code() {
        let result = CommandResult {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: "signal".to_string(),
            duration_ms: 1,
        };
        let err = result.ok().unwrap_err();
        assert!(err.to_string().contains("None"));
    }

    #[test]
    fn command_result_from_output_success() {
        let output = std::process::Output {
            status: make_exit_status(0),
            stdout: b"out".to_vec(),
            stderr: b"err".to_vec(),
        };
        let r = CommandResult::from_output(&output, Duration::from_millis(250));
        assert!(r.success);
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.stdout, "out");
        assert_eq!(r.stderr, "err");
        assert_eq!(r.duration_ms, 250);
    }

    #[test]
    fn command_result_from_output_failure() {
        let output = std::process::Output {
            status: make_exit_status(1),
            stdout: Vec::new(),
            stderr: b"fail".to_vec(),
        };
        let r = CommandResult::from_output(&output, Duration::from_millis(50));
        assert!(!r.success);
        assert_eq!(r.exit_code, Some(1));
        assert_eq!(r.stderr, "fail");
    }

    #[test]
    fn command_result_deserialization() {
        let json = r#"{
            "success": false,
            "exit_code": 7,
            "stdout": "hi",
            "stderr": "lo",
            "duration_ms": 99
        }"#;
        let r: CommandResult = serde_json::from_str(json).expect("deser");
        assert!(!r.success);
        assert_eq!(r.exit_code, Some(7));
        assert_eq!(r.stdout, "hi");
        assert_eq!(r.stderr, "lo");
        assert_eq!(r.duration_ms, 99);
    }

    #[test]
    fn command_result_roundtrip_serde() {
        let original = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: "data\nwith\nnewlines".to_string(),
            stderr: String::new(),
            duration_ms: 1000,
        };
        let json = serde_json::to_string(&original).expect("ser");
        let decoded: CommandResult = serde_json::from_str(&json).expect("deser");
        assert_eq!(decoded.success, original.success);
        assert_eq!(decoded.exit_code, original.exit_code);
        assert_eq!(decoded.stdout, original.stdout);
        assert_eq!(decoded.duration_ms, original.duration_ms);
    }

    // ── run_command tests ─────────────────────────────────────────────

    #[test]
    fn run_command_captures_stdout() {
        // `cargo --version` writes to stdout
        let r = run_command("cargo", &["--version"]).expect("run");
        assert!(!r.stdout.is_empty());
        assert!(r.stdout.starts_with("cargo"));
    }

    #[test]
    fn run_command_captures_stderr_on_failure() {
        let r = run_command("cargo", &["publish", "--help-not-real"]).expect("run");
        // cargo writes error text to stderr for unknown flags
        assert!(!r.success);
        assert!(!r.stderr.is_empty());
    }

    #[test]
    fn run_command_records_duration() {
        let r = run_command("cargo", &["--version"]).expect("run");
        // duration should be non-negative (and realistically > 0)
        assert!(r.duration_ms < 30_000, "took too long: {}ms", r.duration_ms);
    }

    #[test]
    fn run_command_nonexistent_program() {
        let err = run_command("totally-bogus-command-xyz-999", &[]);
        assert!(err.is_err(), "should fail for non-existent program");
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("failed to run command"),
            "unexpected error: {msg}"
        );
    }

    // ── run_command_in_dir tests ──────────────────────────────────────

    #[test]
    fn run_command_in_dir_uses_working_dir() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        // `cmd /C cd` on Windows prints the current directory
        #[cfg(windows)]
        {
            let r = run_command_in_dir("cmd", &["/C", "cd"], tmp.path()).expect("run");
            assert!(r.success);
            let normalised = r.stdout.trim().to_lowercase();
            let expected = tmp.path().to_str().unwrap().to_lowercase();
            assert!(
                normalised.contains(&expected),
                "stdout={normalised:?} expected to contain {expected:?}"
            );
        }
        #[cfg(not(windows))]
        {
            let r = run_command_in_dir("pwd", &[], tmp.path()).expect("run");
            assert!(r.success);
            assert!(
                r.stdout
                    .trim()
                    .ends_with(tmp.path().file_name().unwrap().to_str().unwrap())
            );
        }
    }

    #[test]
    fn run_command_in_dir_nonexistent_dir() {
        let bad = std::path::Path::new("Z:\\this\\path\\does\\not\\exist\\at\\all");
        let err = run_command_in_dir("cargo", &["--version"], bad);
        assert!(err.is_err());
    }

    // ── run_command_with_env tests ────────────────────────────────────

    #[test]
    fn run_command_with_env_passes_variables() {
        #[cfg(windows)]
        {
            let r = run_command_with_env(
                "cmd",
                &["/C", "echo %SHIPPER_TEST_VAR%"],
                &[("SHIPPER_TEST_VAR".to_string(), "hello42".to_string())],
            )
            .expect("run");
            assert!(r.success);
            assert!(
                r.stdout.contains("hello42"),
                "stdout should contain env value: {:?}",
                r.stdout
            );
        }
        #[cfg(not(windows))]
        {
            let r = run_command_with_env(
                "sh",
                &["-c", "echo $SHIPPER_TEST_VAR"],
                &[("SHIPPER_TEST_VAR".to_string(), "hello42".to_string())],
            )
            .expect("run");
            assert!(r.success);
            assert!(r.stdout.contains("hello42"));
        }
    }

    #[test]
    fn run_command_with_env_multiple_vars() {
        #[cfg(windows)]
        {
            let r = run_command_with_env(
                "cmd",
                &["/C", "echo %A% %B%"],
                &[
                    ("A".to_string(), "foo".to_string()),
                    ("B".to_string(), "bar".to_string()),
                ],
            )
            .expect("run");
            assert!(r.success);
            assert!(r.stdout.contains("foo"));
            assert!(r.stdout.contains("bar"));
        }
        #[cfg(not(windows))]
        {
            let r = run_command_with_env(
                "sh",
                &["-c", "echo $A $B"],
                &[
                    ("A".to_string(), "foo".to_string()),
                    ("B".to_string(), "bar".to_string()),
                ],
            )
            .expect("run");
            assert!(r.success);
            assert!(r.stdout.contains("foo"));
            assert!(r.stdout.contains("bar"));
        }
    }

    // ── run_command_simple tests ──────────────────────────────────────

    #[test]
    fn run_command_simple_returns_false_on_failure() {
        let ok = run_command_simple("cargo", &["--nonexistent-flag-xyz"]).expect("run");
        assert!(!ok);
    }

    #[test]
    fn run_command_simple_nonexistent_program() {
        let err = run_command_simple("bogus-not-a-command-123", &[]);
        assert!(err.is_err());
    }

    // ── run_command_streaming tests ───────────────────────────────────

    #[test]
    fn run_command_streaming_success() {
        // stdout/stderr are inherited so captured strings are empty,
        // but the command should still succeed.
        let r = run_command_streaming("cargo", &["--version"]).expect("run");
        assert!(r.success);
        assert_eq!(r.exit_code, Some(0));
    }

    #[test]
    fn run_command_streaming_failure() {
        let r = run_command_streaming("cargo", &["--nonexistent-flag-xyz"]).expect("run");
        assert!(!r.success);
    }

    // ── run_command_with_timeout tests ────────────────────────────────

    #[test]
    fn run_command_with_timeout_none_delegates() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let r = run_command_with_timeout("cargo", &["--version"], tmp.path(), None).expect("run");
        assert!(!r.timed_out);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("cargo"));
    }

    #[test]
    fn run_command_with_timeout_completes_before_deadline() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let timeout = Some(Duration::from_secs(30));
        let r =
            run_command_with_timeout("cargo", &["--version"], tmp.path(), timeout).expect("run");
        assert!(!r.timed_out);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("cargo"));
    }

    #[test]
    fn run_command_with_timeout_exceeds_deadline() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        // Use a very short timeout so the child is killed quickly
        let timeout = Some(Duration::from_millis(100));

        #[cfg(windows)]
        let r = run_command_with_timeout("ping", &["-n", "100", "127.0.0.1"], tmp.path(), timeout)
            .expect("run");
        #[cfg(not(windows))]
        let r = run_command_with_timeout("sleep", &["60"], tmp.path(), timeout).expect("run");

        assert!(r.timed_out, "should have timed out");
        assert_eq!(r.exit_code, -1);
        assert!(
            r.stderr.contains("timed out"),
            "stderr should mention timeout: {:?}",
            r.stderr
        );
    }

    #[test]
    fn run_command_with_timeout_failure_before_deadline() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let timeout = Some(Duration::from_secs(30));
        let r = run_command_with_timeout("cargo", &["--nonexistent-flag-xyz"], tmp.path(), timeout)
            .expect("run");
        assert!(!r.timed_out);
        assert_ne!(r.exit_code, 0);
    }

    #[test]
    fn run_command_with_timeout_nonexistent_program() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let err = run_command_with_timeout(
            "bogus-not-a-command-123",
            &[],
            tmp.path(),
            Some(Duration::from_secs(5)),
        );
        assert!(err.is_err());
    }

    #[test]
    fn run_command_with_timeout_captures_stderr() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let timeout = Some(Duration::from_secs(30));
        let r = run_command_with_timeout("cargo", &["--nonexistent-flag-xyz"], tmp.path(), timeout)
            .expect("run");
        // cargo should write an error message to stderr for the unknown flag
        assert!(
            !r.stderr.is_empty(),
            "stderr should not be empty on failure"
        );
    }

    // ── CommandOutput tests ──────────────────────────────────────────

    #[test]
    fn command_output_duration_is_reasonable() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let r = run_command_with_timeout("cargo", &["--version"], tmp.path(), None).expect("run");
        assert!(r.duration < Duration::from_secs(30));
    }

    #[test]
    fn command_output_serialization_roundtrip() {
        let co = CommandOutput {
            exit_code: 2,
            stdout: "out".to_string(),
            stderr: "err".to_string(),
            timed_out: true,
            duration: Duration::from_millis(500),
        };
        let json = serde_json::to_string(&co).expect("ser");
        let decoded: CommandOutput = serde_json::from_str(&json).expect("deser");
        assert_eq!(decoded.exit_code, 2);
        assert_eq!(decoded.stdout, "out");
        assert_eq!(decoded.stderr, "err");
        assert!(decoded.timed_out);
        assert_eq!(decoded.duration, Duration::from_millis(500));
    }

    // ── command_exists / which tests ─────────────────────────────────

    #[test]
    fn which_nonexistent_returns_none() {
        assert!(which("this-command-does-not-exist-xyz123").is_none());
    }

    #[test]
    fn which_cargo_returns_valid_path() {
        let p = which("cargo").expect("cargo should be in PATH");
        assert!(p.exists(), "path should exist: {}", p.display());
    }

    // ── run_cargo helpers ────────────────────────────────────────────

    #[test]
    fn run_cargo_in_dir_works() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let r = run_cargo_in_dir(&["--version"], tmp.path()).expect("run");
        assert!(r.success);
        assert!(r.stdout.contains("cargo"));
    }

    #[test]
    fn run_cargo_failure() {
        let r = run_cargo(&["--nonexistent-flag-xyz"]).expect("run");
        assert!(!r.success);
    }

    // ── Exit code tests ──────────────────────────────────────────────

    #[test]
    fn exit_code_zero_on_success() {
        let r = run_command("cargo", &["--version"]).expect("run");
        assert_eq!(r.exit_code, Some(0));
    }

    #[test]
    fn exit_code_nonzero_on_failure() {
        let r = run_command("cargo", &["--nonexistent-flag-xyz"]).expect("run");
        assert!(r.exit_code.is_some());
        assert_ne!(r.exit_code.unwrap(), 0);
    }

    #[test]
    fn specific_exit_code() {
        #[cfg(windows)]
        {
            let r = run_command("cmd", &["/C", "exit 42"]).expect("run");
            assert_eq!(r.exit_code, Some(42));
            assert!(!r.success);
        }
        #[cfg(not(windows))]
        {
            let r = run_command("sh", &["-c", "exit 42"]).expect("run");
            assert_eq!(r.exit_code, Some(42));
            assert!(!r.success);
        }
    }

    // ── Property-based tests (proptest) ─────────────────────────────

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;

        // ── Process output handling with arbitrary stdout/stderr ──

        proptest! {
            #[test]
            fn command_result_ok_succeeds_when_success_is_true(
                stdout in any::<String>(),
                stderr in any::<String>(),
                exit_code in proptest::option::of(any::<i32>()),
                duration_ms in any::<u64>(),
            ) {
                let result = CommandResult {
                    success: true,
                    exit_code,
                    stdout,
                    stderr,
                    duration_ms,
                };
                prop_assert!(result.ok().is_ok());
            }

            #[test]
            fn command_result_ok_fails_when_success_is_false(
                stdout in any::<String>(),
                stderr in any::<String>(),
                exit_code in proptest::option::of(any::<i32>()),
                duration_ms in any::<u64>(),
            ) {
                let result = CommandResult {
                    success: false,
                    exit_code,
                    stdout,
                    stderr: stderr.clone(),
                    duration_ms,
                };
                let err = result.ok().unwrap_err();
                let msg = err.to_string();
                prop_assert!(msg.contains(&stderr));
            }

            #[test]
            fn command_result_serde_roundtrip(
                success in any::<bool>(),
                exit_code in proptest::option::of(any::<i32>()),
                stdout in any::<String>(),
                stderr in any::<String>(),
                duration_ms in any::<u64>(),
            ) {
                let original = CommandResult {
                    success,
                    exit_code,
                    stdout: stdout.clone(),
                    stderr: stderr.clone(),
                    duration_ms,
                };
                let json = serde_json::to_string(&original).unwrap();
                let decoded: CommandResult = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(decoded.success, success);
                prop_assert_eq!(decoded.exit_code, exit_code);
                prop_assert_eq!(&decoded.stdout, &stdout);
                prop_assert_eq!(&decoded.stderr, &stderr);
                prop_assert_eq!(decoded.duration_ms, duration_ms);
            }

            #[test]
            fn command_output_serde_roundtrip(
                exit_code in any::<i32>(),
                stdout in any::<String>(),
                stderr in any::<String>(),
                timed_out in any::<bool>(),
                duration_ms in 0u64..=u64::MAX / 1_000_000,
            ) {
                let original = CommandOutput {
                    exit_code,
                    stdout: stdout.clone(),
                    stderr: stderr.clone(),
                    timed_out,
                    duration: Duration::from_millis(duration_ms),
                };
                let json = serde_json::to_string(&original).unwrap();
                let decoded: CommandOutput = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(decoded.exit_code, exit_code);
                prop_assert_eq!(&decoded.stdout, &stdout);
                prop_assert_eq!(&decoded.stderr, &stderr);
                prop_assert_eq!(decoded.timed_out, timed_out);
                prop_assert_eq!(decoded.duration, Duration::from_millis(duration_ms));
            }
        }

        // ── Exit code interpretation (arbitrary i32 exit codes) ──

        proptest! {
            #[test]
            fn error_message_contains_exit_code(code in any::<i32>()) {
                let result = CommandResult {
                    success: false,
                    exit_code: Some(code),
                    stdout: String::new(),
                    stderr: String::new(),
                    duration_ms: 0,
                };
                let err = result.ok().unwrap_err();
                let msg = err.to_string();
                let code_str = code.to_string();
                prop_assert!(msg.contains(&code_str));
            }

            #[test]
            fn error_message_contains_none_when_exit_code_missing(
                stderr in any::<String>(),
            ) {
                let result = CommandResult {
                    success: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr,
                    duration_ms: 0,
                };
                let err = result.ok().unwrap_err();
                prop_assert!(err.to_string().contains("None"));
            }

            #[test]
            fn exit_code_zero_is_always_success(
                stdout in any::<String>(),
                stderr in any::<String>(),
                duration_ms in any::<u64>(),
            ) {
                let result = CommandResult {
                    success: true,
                    exit_code: Some(0),
                    stdout,
                    stderr,
                    duration_ms,
                };
                prop_assert!(result.ok().is_ok());
                prop_assert_eq!(result.exit_code, Some(0));
            }

            #[test]
            fn nonzero_exit_code_produces_error(code in any::<i32>().prop_filter(
                "non-zero exit code",
                |c| *c != 0,
            )) {
                let result = CommandResult {
                    success: false,
                    exit_code: Some(code),
                    stdout: String::new(),
                    stderr: "failed".to_string(),
                    duration_ms: 0,
                };
                prop_assert!(result.ok().is_err());
            }
        }

        // ── Command building with arbitrary argument lists ──

        proptest! {
            #[test]
            fn command_building_does_not_panic(
                args in proptest::collection::vec(any::<String>(), 0..20),
            ) {
                let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                let mut cmd = Command::new("echo");
                cmd.args(&arg_refs);
                // Building a command must never panic regardless of arguments
                prop_assert!(true);
            }

            #[test]
            fn command_with_env_building_does_not_panic(
                args in proptest::collection::vec(any::<String>(), 0..10),
                env_keys in proptest::collection::vec("[A-Z_]{1,20}", 0..5),
                env_vals in proptest::collection::vec(any::<String>(), 0..5),
            ) {
                let mut cmd = Command::new("echo");
                let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                cmd.args(&arg_refs);
                let pairs = env_keys.len().min(env_vals.len());
                for i in 0..pairs {
                    cmd.env(&env_keys[i], &env_vals[i]);
                }
                prop_assert!(true);
            }
        }
    }

    // ── Helper to create ExitStatus with a given code ────────────────

    /// Create an `ExitStatus` by actually running a process that exits with the given code.
    fn make_exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(windows)]
        {
            Command::new("cmd")
                .args(["/C", &format!("exit {code}")])
                .status()
                .expect("cmd exit")
        }
        #[cfg(not(windows))]
        {
            Command::new("sh")
                .args(["-c", &format!("exit {code}")])
                .status()
                .expect("sh exit")
        }
    }
}
