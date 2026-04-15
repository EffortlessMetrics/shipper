//! Basic command runners without timeout.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use super::types::CommandResult;

/// Run a command and capture its output.
#[allow(dead_code)]
pub(crate) fn run_command(program: &str, args: &[&str]) -> Result<CommandResult> {
    let start = std::time::Instant::now();

    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run command: {} {:?}", program, args))?;

    Ok(CommandResult::from_output(&output, start.elapsed()))
}

/// Run a command in a specific directory.
#[allow(dead_code)]
pub(crate) fn run_command_in_dir(
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

/// Run a command with environment variables.
#[allow(dead_code)]
pub(crate) fn run_command_with_env(
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

/// Run a command and stream output to stdout/stderr.
#[allow(dead_code)]
pub(crate) fn run_command_streaming(program: &str, args: &[&str]) -> Result<CommandResult> {
    let start = std::time::Instant::now();

    let output = Command::new(program)
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("failed to run command: {} {:?}", program, args))?;

    Ok(CommandResult::from_output(&output, start.elapsed()))
}

/// Run a command and return success/failure without capturing output.
#[allow(dead_code)]
pub(crate) fn run_command_simple(program: &str, args: &[&str]) -> Result<bool> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run command: {} {:?}", program, args))?;

    Ok(status.success())
}
