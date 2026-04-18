//! Command execution with optional timeout.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::run::run_command_in_dir;
use super::types::CommandOutput;

/// Run a command with optional timeout and captured output.
#[allow(dead_code)]
pub(crate) fn run_command_with_timeout(
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
