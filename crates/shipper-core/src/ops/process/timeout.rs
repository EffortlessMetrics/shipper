//! Command execution with optional timeout.

use std::time::Duration;

use anyhow::{Result, bail};

use super::CapturedCommandOutput;
use super::capture::CaptureMode;
use super::interruption::InterruptionLease;
use super::run::run_command_in_dir;
use super::types::CommandOutput;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use unix::run_supervised;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::run_supervised;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod legacy;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
use legacy::run_supervised;

/// Run a command with optional timeout and captured output.
#[allow(dead_code)]
pub(crate) fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    working_dir: &std::path::Path,
    timeout: Option<Duration>,
) -> Result<CommandOutput> {
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

    let captured = run_supervised(
        program,
        args,
        working_dir,
        Some(timeout_dur),
        CaptureMode::Full,
    )?;
    if let Some(failure) = captured.cleanup_failure {
        bail!("command cleanup incomplete: {}", failure.description());
    }
    if captured.interrupted || super::interruption::requested() {
        return Err(super::interruption::Interrupted.into());
    }
    Ok(captured.output)
}

/// A supervisor proved that the invocation never launched Cargo. The error
/// carries its lease through the caller's durable no-launch checkpoint.
pub(crate) struct CommandNotLaunched(Option<InterruptionLease>);

impl CommandNotLaunched {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    fn for_publish(
        error: anyhow::Error,
        capture: CaptureMode,
        lease: Option<InterruptionLease>,
    ) -> anyhow::Error {
        if matches!(capture, CaptureMode::PublishTail(_)) {
            return error.context(Self(lease));
        }
        error
    }

    pub(crate) fn take_lease(&mut self) -> Option<InterruptionLease> {
        self.0.take()
    }
}

impl std::fmt::Debug for CommandNotLaunched {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandNotLaunched")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for CommandNotLaunched {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("command was not launched")
    }
}

/// Publish diagnostics opt into bounded retention; generic stdout stays complete.
pub(crate) fn run_command_with_publish_capture(
    program: &str,
    args: &[&str],
    working_dir: &std::path::Path,
    timeout: Option<Duration>,
    output_lines: usize,
) -> Result<CapturedCommandOutput> {
    run_supervised(
        program,
        args,
        working_dir,
        timeout,
        CaptureMode::PublishTail(output_lines),
    )
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod cleanup_tests;

#[cfg(all(test, windows))]
mod windows_tests;
