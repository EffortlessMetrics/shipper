use std::path::Path;
use std::time::Duration;

use anyhow::Result;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct CommandOutput {
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) timed_out: bool,
    pub(crate) duration: Duration,
}

#[allow(dead_code)]
pub(crate) fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    working_dir: &Path,
    timeout: Option<Duration>,
) -> Result<CommandOutput> {
    let output = shipper_process::run_command_with_timeout(program, args, working_dir, timeout)?;

    Ok(CommandOutput {
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out: output.timed_out,
        duration: output.duration,
    })
}
