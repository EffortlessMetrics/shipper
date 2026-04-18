//! Result types for process execution.

use std::process::Output;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Result of a command execution.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CommandResult {
    /// Whether the command succeeded (exit code 0).
    pub(crate) success: bool,
    /// Exit code (if available).
    pub(crate) exit_code: Option<i32>,
    /// Standard output.
    pub(crate) stdout: String,
    /// Standard error.
    pub(crate) stderr: String,
    /// Duration of execution.
    pub(crate) duration_ms: u64,
}

impl CommandResult {
    /// Check if the command succeeded.
    #[allow(dead_code)]
    pub(crate) fn ok(&self) -> Result<&Self> {
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

    /// Create a result from a process output.
    #[allow(dead_code)]
    pub(crate) fn from_output(output: &Output, duration: Duration) -> Self {
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
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CommandOutput {
    /// Exit code (or -1 when not available).
    pub(crate) exit_code: i32,
    /// Captured stdout.
    pub(crate) stdout: String,
    /// Captured stderr.
    pub(crate) stderr: String,
    /// Whether execution exceeded timeout.
    pub(crate) timed_out: bool,
    /// Total wall-clock duration.
    #[allow(dead_code)]
    pub(crate) duration: Duration,
}
