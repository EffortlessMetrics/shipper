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
}
