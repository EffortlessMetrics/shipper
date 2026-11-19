//! Bounded Windows supervision of an invocation-owned Job Object.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::os::windows::io::AsHandle;
use std::path::Path;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use interprocess::os::windows::named_pipe::{PipeListenerOptions, PipeMode, PipeStream, pipe_mode};
use processkit::ProcessGroup;
use windows_spawn::{Child, Command, Stdio};

use super::super::capture::{CaptureBuffer, CaptureMode};
use super::super::interruption::{self, InterruptionLease};
use super::super::{
    CaptureLoss, CaptureStatus, CapturedCommandOutput, CleanupFailure, CommandOutput,
};

use super::CommandNotLaunched;

pub(super) const CLEANUP_ALLOWANCE: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(8);
const READ_BYTES: usize = 8 * 1024;
const READS_PER_TURN: usize = 8;

pub(super) fn run_supervised(
    program: &str,
    args: &[&str],
    working_dir: &Path,
    timeout: Option<Duration>,
    capture: CaptureMode,
) -> Result<CapturedCommandOutput> {
    let mut lease = InterruptionLease::acquire()
        .map_err(|error| CommandNotLaunched::for_publish(error, capture, None))?;
    let mut before_launch = |error| CommandNotLaunched::for_publish(error, capture, lease.take());
    interruption::check().map_err(&mut before_launch)?;
    let started = Instant::now();
    let execution_deadline = timeout
        .map(|duration| {
            started
                .checked_add(duration)
                .context("command timeout exceeds the monotonic clock range")
        })
        .transpose()
        .map_err(&mut before_launch)?;

    // A fresh, unnamed, kill-on-close job. The direct child owns its
    // process handle and exit status separately from the job.
    let job = create_job().map_err(&mut before_launch)?;
    let (stdout, stdout_writer) = capture_pipe(
        #[cfg(test)]
        Fault::ReadStdout,
    )
    .context("failed to prepare Windows stdout")
    .map_err(&mut before_launch)?;
    let (stderr, stderr_writer) = capture_pipe(
        #[cfg(test)]
        Fault::ReadStderr,
    )
    .context("failed to prepare Windows stderr")
    .map_err(&mut before_launch)?;
    let mut command = command_for(program, args, working_dir)
        .with_context(|| format!("failed to prepare Windows command: {program}"))
        .map_err(&mut before_launch)?;
    command
        .current_dir(working_dir)
        .stdout(Stdio::from(stdout_writer))
        .stderr(Stdio::from(stderr_writer));

    interruption::check().map_err(&mut before_launch)?;
    let mut child = match spawn_in_job(&job, command) {
        Ok(child) => child,
        Err(error) => {
            let failure = match error {
                LaunchError::Create(error) => {
                    // In windows-spawn 0.1.0, CreateProcessW is the last
                    // fallible step before returning the suspended handles.
                    return Err(error)
                        .with_context(|| format!("failed to spawn command: {program}"))
                        .map_err(before_launch);
                }
                LaunchError::Assign => CleanupFailure::JobAssignment,
                LaunchError::Resume => CleanupFailure::JobResume,
            };
            // A suspended child may have been created. The wrapper's
            // rollback attempts termination without confirming exit.
            return Ok(CapturedCommandOutput {
                output: CommandOutput {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: false,
                    duration: started.elapsed(),
                },
                capture: CaptureStatus {
                    stdout: Some(CaptureLoss::Cleanup),
                    stderr: Some(CaptureLoss::Cleanup),
                },
                cleanup_failure: Some(failure),
                interrupted: interruption::requested(),
                interruption_lease: lease,
            });
        }
    };
    let mut stdout = PipeCapture::new(stdout, capture);
    let mut stderr = PipeCapture::new(stderr, capture);
    let mut cleanup_failure = None;
    let mut exit_status = None;
    let mut timed_out = false;
    let mut interrupted = false;
    let mut cleanup = None;

    loop {
        let made_progress = stdout.drain_turn() | stderr.drain_turn();
        interrupted |= interruption::requested();
        let now = Instant::now();

        if cleanup.is_none() {
            match child.try_wait() {
                Ok(status) => exit_status = status,
                Err(_) => cleanup_failure = Some(CleanupFailure::ChildStatus),
            }
            if exit_status.is_none() && cleanup_failure.is_none() && !interrupted {
                timed_out = execution_deadline.is_some_and(|deadline| now >= deadline);
            }
            if exit_status.is_some() || cleanup_failure.is_some() || interrupted || timed_out {
                // Termination initiation and completion are separate. Even a
                // failed termination can be followed by confirmed empty state.
                cleanup = Some((now + CLEANUP_ALLOWANCE, !terminate(&job)));
            }
        }

        if let Some((deadline, termination_failed)) = cleanup {
            let reaped = poll_reap(&mut child, exit_status.is_some(), &mut cleanup_failure);
            let empty = job_empty(&job);
            if reaped && empty && stdout.reader_closed() && stderr.reader_closed() {
                if !stdout.eof_confirmed || !stderr.eof_confirmed {
                    cleanup_failure.get_or_insert(CleanupFailure::PipeEofUnconfirmed);
                }
                break;
            }
            if Instant::now() >= deadline {
                if !reaped {
                    cleanup_failure.get_or_insert(CleanupFailure::Reap);
                }
                if !empty {
                    cleanup_failure.get_or_insert(if termination_failed {
                        CleanupFailure::JobTermination
                    } else {
                        CleanupFailure::JobStillActive
                    });
                }
                if !stdout.reader_closed() || !stderr.reader_closed() {
                    cleanup_failure.get_or_insert(CleanupFailure::InheritedPipe);
                    stdout.cancel();
                    stderr.cancel();
                }
                break;
            }
        }

        if !made_progress {
            let remaining = cleanup
                .map(|(deadline, _)| deadline)
                .or(execution_deadline)
                .map_or(POLL_INTERVAL, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });
            thread::sleep(POLL_INTERVAL.min(remaining));
        }
    }

    // Close the last job handle before handing the lease to the caller. This
    // also issues kill-on-close if explicit termination could not be confirmed.
    drop(child);
    drop(job);
    if let Some(duration) = timeout.filter(|_| timed_out) {
        stderr.buffer.push(
            format!(
                "\n{program} timed out after {}",
                humantime::format_duration(duration)
            )
            .as_bytes(),
        );
    }
    let (stdout, stdout_loss) = stdout.buffer.finish();
    let (stderr, stderr_loss) = stderr.buffer.finish();
    Ok(CapturedCommandOutput {
        output: CommandOutput {
            exit_code: exit_status
                .and_then(|status: ExitStatus| status.code())
                .unwrap_or(-1),
            stdout,
            stderr,
            timed_out,
            duration: started.elapsed(),
        },
        capture: CaptureStatus {
            stdout: stdout_loss,
            stderr: stderr_loss,
        },
        cleanup_failure,
        interrupted: interrupted || interruption::requested(),
        interruption_lease: lease,
    })
}

fn command_for(program: &str, args: &[&str], working_dir: &Path) -> io::Result<Command> {
    let path = Path::new(program);
    if !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
    {
        let mut command = Command::new(program);
        command.args(args);
        return Ok(command);
    }

    let script = which::which_in(program, std::env::var_os("PATH"), working_dir)
        .map_err(|error| io::Error::new(ErrorKind::NotFound, error))?;
    let script = script.to_str().ok_or_else(invalid_batch_argument)?;
    if script.contains(['%', '"', '\0', '\r', '\n']) {
        return Err(invalid_batch_argument());
    }

    let system_root = std::env::var_os("SystemRoot")
        .context("Windows system directory is unavailable")
        .map_err(io::Error::other)?;
    let mut command = Command::new(Path::new(&system_root).join("System32").join("cmd.exe"));
    let mut command_line = format!("/d /e:ON /v:OFF /s /c \"\"{script}\"");
    for arg in args {
        command_line.push(' ');
        append_batch_argument(&mut command_line, arg)?;
    }
    command_line.push('"');
    command.raw_arg(command_line);
    Ok(command)
}

fn invalid_batch_argument() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        "invalid Windows batch path or argument",
    )
}

fn append_batch_argument(command_line: &mut String, arg: &str) -> io::Result<()> {
    if arg.contains(['\0', '\r', '\n']) {
        return Err(invalid_batch_argument());
    }
    // Match the Rust Windows batch argument rules for UTF-8 arguments.
    // https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/args/windows.rs
    let quoted = arg.is_empty()
        || arg.ends_with('\\')
        || arg.chars().any(|character| {
            character.is_control()
                || (character.is_ascii()
                    && !character.is_ascii_alphanumeric()
                    && !r"#$*+-./:?@\_".contains(character))
        });
    if quoted {
        command_line.push('"');
    }
    let mut slashes = 0;
    for character in arg.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                command_line.extend(std::iter::repeat_n('\\', slashes));
                command_line.push('"');
                slashes = 0;
            }
            '%' => {
                // Consume one empty cmd.exe substring expansion to preserve
                // the percent sign as data instead of expanding a variable.
                command_line.push_str("%%cd:~,");
                slashes = 0;
            }
            _ => slashes = 0,
        }
        command_line.push(character);
    }
    if quoted {
        command_line.extend(std::iter::repeat_n('\\', slashes));
        command_line.push('"');
    }
    Ok(())
}

struct PipeReader {
    probe: PipeStream<pipe_mode::Messages, pipe_mode::Messages>,
    bytes: PipeStream<pipe_mode::Bytes, pipe_mode::Bytes>,
    #[cfg(test)]
    read_fault: Fault,
}

impl Read for PipeReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        #[cfg(test)]
        if has_fault(self.read_fault) {
            return Err(io::Error::other("injected output read failure"));
        }
        // Read in byte mode so zero-length messages cannot mask later bytes.
        // interprocess normalizes both ERROR_NO_DATA and ERROR_BROKEN_PIPE
        // to zero here. PeekNamedPipe distinguishes open-empty from EOF.
        let count = self.bytes.read(buffer)?;
        if count != 0 {
            return Ok(count);
        }
        match self.probe.peek_msg_len() {
            Ok(_) => Err(ErrorKind::WouldBlock.into()),
            Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(0),
            Err(error) => Err(error),
        }
    }
}

fn capture_pipe(#[cfg(test)] read_fault: Fault) -> io::Result<(PipeReader, File)> {
    let name = format!(r"\\.\pipe\shipper-{:032x}", rand::random::<u128>());
    let listener = PipeListenerOptions::new()
        .path(name.as_str())
        .mode(PipeMode::Messages)
        .nonblocking(true)
        .input_buffer_size_hint(64 * 1024)
        .create_duplex::<pipe_mode::Messages>()?;
    let writer = OpenOptions::new().write(true).open(&name)?;
    let probe = listener.accept()?;
    // accept installs another listening instance. Close it before conversion
    // tries ReOpenFile, so the byte view cannot select the unconnected instance.
    drop(listener);
    let bytes = PipeStream::try_from(probe.as_handle().try_clone_to_owned()?)
        .map_err(|error| io::Error::other(error.to_string()))?;
    // The second view reads bytes and never writes. Prevent the conversion's
    // default flush-on-drop, and keep the OS read mode nonblocking.
    bytes.assume_flushed();
    bytes.set_nonblocking(true)?;
    Ok((
        PipeReader {
            probe,
            bytes,
            #[cfg(test)]
            read_fault,
        },
        writer,
    ))
}

fn poll_reap(
    child: &mut Child,
    already_reaped: bool,
    failure: &mut Option<CleanupFailure>,
) -> bool {
    #[cfg(test)]
    if has_fault(Fault::Reap) {
        return false;
    }
    if already_reaped {
        return true;
    }
    match child.try_wait() {
        Ok(status) => status.is_some(),
        Err(_) => {
            failure.get_or_insert(CleanupFailure::Reap);
            false
        }
    }
}

fn create_job() -> Result<ProcessGroup> {
    #[cfg(test)]
    if has_fault(Fault::JobSetup) {
        anyhow::bail!("failed to create the invocation's Windows Job Object");
    }
    ProcessGroup::new().context("failed to create the invocation's Windows Job Object")
}

#[derive(Debug)]
pub(super) enum LaunchError {
    Create(io::Error),
    Assign,
    Resume,
}

pub(super) fn spawn_in_job(
    job: &ProcessGroup,
    mut command: Command,
) -> std::result::Result<Child, LaunchError> {
    let suspended = command.spawn_suspended().map_err(LaunchError::Create)?;
    // The process handle retained by 'suspended' pins this invocation's PID
    // across ProcessKit's OpenProcess/AssignProcessToJobObject call. That API
    // may also succeed if the process already exited, so require a live member
    // in the otherwise empty job before attempting resume.
    #[cfg(test)]
    if has_fault(Fault::Assign) {
        return Err(LaunchError::Assign);
    }
    job.adopt_external(suspended.id())
        .map_err(|_| LaunchError::Assign)?;
    if !job
        .stats()
        .is_ok_and(|stats| stats.active_process_count == 1)
    {
        return Err(LaunchError::Assign);
    }
    #[cfg(test)]
    if has_fault(Fault::Resume) {
        return Err(LaunchError::Resume);
    }
    // windows-spawn resumes only the primary-thread handle returned by the
    // same CreateProcessW call. ProcessKit never enumerates or resumes threads.
    suspended.resume().map_err(|_| LaunchError::Resume)
}

fn terminate(job: &ProcessGroup) -> bool {
    #[cfg(test)]
    if has_fault(Fault::Terminate) {
        return false;
    }
    job.kill_all().is_ok()
}

fn job_empty(job: &ProcessGroup) -> bool {
    #[cfg(test)]
    if has_fault(Fault::Accounting) {
        return false;
    }
    job.stats()
        .is_ok_and(|stats| stats.active_process_count == 0)
}

struct PipeCapture<R> {
    stream: Option<R>,
    eof_confirmed: bool,
    buffer: CaptureBuffer,
}

impl<R: Read> PipeCapture<R> {
    fn new(stream: R, capture: CaptureMode) -> Self {
        Self {
            stream: Some(stream),
            eof_confirmed: false,
            buffer: CaptureBuffer::new(capture),
        }
    }

    fn drain_turn(&mut self) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        let mut chunk = [0; READ_BYTES];
        let mut made_progress = false;
        for _ in 0..READS_PER_TURN {
            let bytes = match stream.read(&mut chunk) {
                Ok(0) => {
                    self.eof_confirmed = true;
                    self.stream = None;
                    break;
                }
                Ok(len) => chunk.get(..len),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => None,
            };
            let Some(bytes) = bytes else {
                self.buffer.lose(CaptureLoss::Read);
                self.stream = None;
                break;
            };
            self.buffer.push(bytes);
            made_progress = true;
        }
        made_progress
    }

    fn reader_closed(&self) -> bool {
        self.stream.is_none()
    }

    fn cancel(&mut self) {
        if self.stream.take().is_some() {
            // The polling reader has no operation outstanding. Dropping it
            // closes the owned handle without waiting for a writer or EOF.
            self.buffer.lose(CaptureLoss::Cleanup);
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Fault {
    JobSetup,
    Assign,
    Resume,
    Terminate,
    Accounting,
    Reap,
    ReadStdout,
    ReadStderr,
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn has_fault(fault: Fault) -> bool {
    FAULT.get() == Some(fault)
}

#[cfg(test)]
pub(super) fn with_fault<T>(fault: Fault, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAULT.set(None);
        }
    }
    assert!(FAULT.replace(Some(fault)).is_none());
    let _reset = Reset;
    run()
}
