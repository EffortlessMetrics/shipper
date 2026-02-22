use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct CommandOutput {
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) timed_out: bool,
    #[allow(dead_code)]
    pub(crate) duration: Duration,
}

#[allow(dead_code)]
pub(crate) fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    working_dir: &Path,
    timeout: Option<Duration>,
) -> Result<CommandOutput> {
    let start = Instant::now();
    let mut command = Command::new(program);
    command.args(args).current_dir(working_dir);

    let (exit_code, stdout, stderr, timed_out) = if let Some(timeout_dur) = timeout {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn command")?;

        let deadline = Instant::now() + timeout_dur;
        loop {
            match child.try_wait().context("failed to poll command")? {
                Some(status) => {
                    let mut stdout_bytes = Vec::new();
                    let mut stderr_bytes = Vec::new();
                    if let Some(mut out) = child.stdout.take() {
                        let _ = out.read_to_end(&mut stdout_bytes);
                    }
                    if let Some(mut err) = child.stderr.take() {
                        let _ = err.read_to_end(&mut stderr_bytes);
                    }
                    break (
                        status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&stdout_bytes).to_string(),
                        String::from_utf8_lossy(&stderr_bytes).to_string(),
                        false,
                    );
                }
                None => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();

                        let mut stdout_bytes = Vec::new();
                        let mut stderr_bytes = Vec::new();
                        if let Some(mut out) = child.stdout.take() {
                            let _ = out.read_to_end(&mut stdout_bytes);
                        }
                        if let Some(mut err) = child.stderr.take() {
                            let _ = err.read_to_end(&mut stderr_bytes);
                        }

                        let mut stderr_str = String::from_utf8_lossy(&stderr_bytes).to_string();
                        stderr_str.push_str(&format!(
                            "\ncommand timed out after {}",
                            humantime::format_duration(timeout_dur)
                        ));
                        break (
                            -1,
                            String::from_utf8_lossy(&stdout_bytes).to_string(),
                            stderr_str,
                            true,
                        );
                    }

                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    } else {
        let output = command.output().context("failed to execute command")?;

        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            false,
        )
    };

    Ok(CommandOutput {
        exit_code,
        stdout,
        stderr,
        timed_out,
        duration: start.elapsed(),
    })
}
