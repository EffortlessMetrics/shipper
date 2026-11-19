//! Finite cleanup of cooperative descendants in an invocation-owned group.
//!
//! Nonblocking reads avoid both pipe backpressure and abandoned reader threads.
//! The unreaped direct child pins the group ID until its single shutdown signal;
//! no destructive operation uses that numeric ID after the child is reaped.
//! Deliberate process-group/session escape is outside this containment boundary.

use std::io::{ErrorKind, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{
    Pid, Signal, WaitId, WaitIdOptions, kill_process_group, test_kill_process_group, waitid,
};

use super::super::capture::{CaptureBuffer, CaptureMode};
use super::super::interruption::InterruptionLease;
use super::super::{
    CaptureLoss, CaptureStatus, CapturedCommandOutput, CleanupFailure, CommandOutput,
};

/// This allowance starts at the first observed exit or execution deadline.
/// It bounds pipe drain and direct-child reaping independently of run time.
pub(super) const CLEANUP_ALLOWANCE: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const READ_BYTES: usize = 8 * 1024;
// Limit each turn so a continuously writable stream cannot starve the other
// stream, status observation, or either deadline.
const READS_PER_TURN: usize = 8;

pub(super) fn run_supervised(
    program: &str,
    args: &[&str],
    working_dir: &Path,
    timeout: Option<Duration>,
    capture: CaptureMode,
) -> Result<CapturedCommandOutput> {
    let interruption_lease = super::super::interruption::InterruptionLease::acquire()?;
    super::super::interruption::check()?;
    let started = Instant::now();
    let execution_deadline = timeout
        .map(|duration| {
            started
                .checked_add(duration)
                .context("command timeout exceeds the monotonic clock range")
        })
        .transpose()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn command: {program}"))?;
    Ok(supervise_group(
        &mut OwnedGroup::new(child),
        program,
        started,
        execution_deadline,
        timeout,
        capture,
        interruption_lease,
    ))
}

fn supervise_group(
    child: &mut OwnedGroup,
    program: &str,
    started: Instant,
    execution_deadline: Option<Instant>,
    timeout: Option<Duration>,
    capture: CaptureMode,
    interruption_lease: Option<InterruptionLease>,
) -> CapturedCommandOutput {
    let (mut cleanup_deadline, mut cleanup_failure) = if child.pid.is_none() {
        // We own the Child, but have no usable group identity. Only attempt
        // direct-child termination, before any reap, and retain a safety hold
        // even if the direct child and both pipes are cleaned up.
        let deadline = Instant::now() + CLEANUP_ALLOWANCE;
        let _ = child.child.kill();
        (Some(deadline), Some(CleanupFailure::ChildStatus))
    } else {
        (None, None)
    };
    let mut stdout = PipeCapture::new(child.child.stdout.take(), capture);
    let mut stderr = PipeCapture::new(child.child.stderr.take(), capture);
    let mut group_signal_failed = false;
    let mut group_gone = false;
    let mut exit_code = -1;
    let mut timed_out = false;
    let mut interrupted = false;

    loop {
        let made_progress = stdout.drain_turn() | stderr.drain_turn();
        interrupted |= super::super::interruption::requested();

        if cleanup_deadline.is_none() {
            // Unlike Child::try_wait, NOWAIT observes an exit without releasing
            // the leader's PID. Keep that identity through group termination.
            match child.observe_exit() {
                Ok(Some(status)) => {
                    exit_code = status;
                    cleanup_deadline = Some(Instant::now() + CLEANUP_ALLOWANCE);
                }
                Ok(None) => {
                    if interrupted {
                        cleanup_deadline = Some(Instant::now() + CLEANUP_ALLOWANCE);
                    } else if execution_deadline.is_some_and(|deadline| Instant::now() >= deadline)
                    {
                        timed_out = true;
                        cleanup_deadline = Some(Instant::now() + CLEANUP_ALLOWANCE);
                    }
                }
                Err(_) => {
                    // With no valid identity observation, do not signal a
                    // potentially recycled PGID. Return a retained safety hold.
                    child.identity_pinned = false;
                    cleanup_failure = Some(CleanupFailure::ChildStatus);
                    cleanup_deadline = Some(Instant::now() + CLEANUP_ALLOWANCE);
                }
            }

            if cleanup_deadline.is_some() && child.identity_pinned && child.signal_group().is_err()
            {
                // A group containing only an exited, unreaped leader may
                // reject a signal on some kernels. Final cleanup facts, not
                // this provisional signal result, decide whether work is held.
                group_signal_failed = true;
            }
        }

        if let Some(deadline) = cleanup_deadline {
            // Never use blocking wait(): a killed child can still be waiting
            // for an OS operation. A failed reap is an explicit refusal result.
            match child.try_reap() {
                Ok(_) => {}
                Err(_) => {
                    cleanup_failure.get_or_insert(CleanupFailure::Reap);
                }
            }
            if child.reaped
                && !group_gone
                && let Some(pid) = child.pid
            {
                // Only observe after reaping: a cached PGID is no longer a safe
                // destructive target. Reuse can conservatively cause a hold,
                // but must never cause us to signal a different invocation.
                group_gone = matches!(test_kill_process_group(pid), Err(rustix::io::Errno::SRCH));
            }
            if child.reaped
                && (group_gone || child.pid.is_none())
                && stdout.finished()
                && stderr.finished()
            {
                break;
            }
            if Instant::now() >= deadline {
                if !child.reaped {
                    cleanup_failure.get_or_insert(CleanupFailure::Reap);
                }
                if !group_gone && child.pid.is_some() {
                    cleanup_failure.get_or_insert(if group_signal_failed {
                        CleanupFailure::GroupSignal
                    } else {
                        CleanupFailure::GroupStillPresent
                    });
                }
                if !stdout.finished() || !stderr.finished() {
                    cleanup_failure.get_or_insert(CleanupFailure::InheritedPipe);
                    stdout.abandon_capture();
                    stderr.abandon_capture();
                }
                break;
            }
        }

        // Busy output yields between bounded reads without sleeping; quiet
        // output uses a short poll delay, also bounded by the cleanup deadline.
        if !made_progress {
            let remaining = cleanup_deadline
                .or(execution_deadline)
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(POLL_INTERVAL);
            thread::sleep(POLL_INTERVAL.min(remaining));
        }
    }

    if let (true, Some(timeout)) = (timed_out, timeout) {
        stderr.buffer.push(
            format!(
                "\n{} timed out after {}",
                program,
                humantime::format_duration(timeout)
            )
            .as_bytes(),
        );
    }
    let (stdout, stdout_loss) = stdout.buffer.finish();
    let (stderr, stderr_loss) = stderr.buffer.finish();
    interrupted |= super::super::interruption::requested();
    // Carry every lease through the caller's immediate outcome decision. A
    // signal can arrive after the last observation above but before handoff;
    // the caller must still own the deferral when it observes that request.
    // Interrupted/uncertain outcomes retain it through evidence persistence.
    CapturedCommandOutput {
        output: CommandOutput {
            exit_code,
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
        interrupted,
        interruption_lease,
    }
}

struct PipeCapture<R> {
    stream: Option<R>,
    buffer: CaptureBuffer,
}

impl<R: Read + AsFd> PipeCapture<R> {
    fn new(stream: Option<R>, capture: CaptureMode) -> Self {
        let mut result = Self {
            stream,
            buffer: CaptureBuffer::new(capture),
        };
        let configured = result.stream.as_ref().is_some_and(|stream| {
            fcntl_getfl(stream)
                .and_then(|flags| fcntl_setfl(stream, flags | OFlags::NONBLOCK))
                .is_ok()
        });
        if !configured {
            result.buffer.lose(CaptureLoss::Read);
            result.stream = None;
        }
        result
    }

    fn drain_turn(&mut self) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        let mut chunk = [0; READ_BYTES];
        let mut made_progress = false;
        for _ in 0..READS_PER_TURN {
            let read = stream.read(&mut chunk).and_then(|len| {
                chunk
                    .get(..len)
                    .ok_or_else(|| ErrorKind::InvalidData.into())
            });
            match read {
                Ok([]) => {
                    self.stream = None;
                    break;
                }
                Ok(bytes) => {
                    self.buffer.push(bytes);
                    made_progress = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.buffer.lose(CaptureLoss::Read);
                    self.stream = None;
                    break;
                }
            }
        }
        made_progress
    }

    fn finished(&self) -> bool {
        self.stream.is_none()
    }

    fn abandon_capture(&mut self) {
        if self.stream.take().is_some() {
            self.buffer.lose(CaptureLoss::Cleanup);
        }
    }
}

struct OwnedGroup {
    child: Child,
    pid: Option<Pid>,
    identity_pinned: bool,
    signalled: bool,
    reaped: bool,
}

impl OwnedGroup {
    fn new(child: Child) -> Self {
        let id = child.id();
        Self::with_id(child, id)
    }

    fn with_id(child: Child, id: u32) -> Self {
        // Never pass 1 to kill_process_group: -1 targets every permitted
        // process rather than an invocation-owned group. Keep Child ownership
        // even if its native group identifier cannot be established.
        let pid = i32::try_from(id)
            .ok()
            .filter(|&raw| raw > 1)
            .and_then(Pid::from_raw);
        Self {
            child,
            pid,
            identity_pinned: pid.is_some(),
            signalled: false,
            reaped: false,
        }
    }

    fn observe_exit(&self) -> rustix::io::Result<Option<i32>> {
        waitid(
            WaitId::Pid(self.pid.ok_or(rustix::io::Errno::INVAL)?),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map(|status| status.map(|status| status.exit_status().unwrap_or(-1)))
    }

    fn signal_group(&mut self) -> rustix::io::Result<()> {
        self.signalled = true;
        #[cfg(test)]
        if INJECT_GROUP_SIGNAL_ERROR.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(rustix::io::Errno::PERM);
        }
        kill_process_group(self.pid.ok_or(rustix::io::Errno::INVAL)?, Signal::KILL)
    }

    fn try_reap(&mut self) -> std::io::Result<()> {
        if !self.reaped && self.child.try_wait()?.is_some() {
            self.reaped = true;
            self.identity_pinned = false;
        }
        Ok(())
    }
}

#[cfg(test)]
static INJECT_GROUP_SIGNAL_ERROR: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(super) struct GroupSignalErrorGuard;

#[cfg(test)]
pub(super) fn inject_group_signal_error() -> GroupSignalErrorGuard {
    assert!(!INJECT_GROUP_SIGNAL_ERROR.swap(true, std::sync::atomic::Ordering::SeqCst));
    GroupSignalErrorGuard
}

#[cfg(test)]
impl Drop for GroupSignalErrorGuard {
    fn drop(&mut self) {
        INJECT_GROUP_SIGNAL_ERROR.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for OwnedGroup {
    fn drop(&mut self) {
        if self.identity_pinned && !self.signalled {
            let _ = self.signal_group();
        }
        // Unwind/setup cleanup must also remain finite. No group signal can
        // occur after this reap. Normal execution already polls through its
        // cleanup allowance and exposes an unsuccessful reap to the caller.
        let _ = self.try_reap();
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io;
    use std::os::fd::BorrowedFd;

    use super::*;

    #[test]
    fn unavailable_group_identity_reaps_direct_child_and_retains_hold() {
        for invalid_id in [0, 1, u32::MAX] {
            // Inject an unavailable group identity, not a different child.
            // This direct-only fixture exits on its own if cleanup regresses.
            let child = Command::new("/bin/sleep")
                .arg("6")
                .process_group(0)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut group = OwnedGroup::with_id(child, invalid_id);
            assert!(group.pid.is_none());
            let result = supervise_group(
                &mut group,
                "/bin/sleep",
                Instant::now(),
                None,
                None,
                CaptureMode::PublishTail(2),
                None,
            );

            assert!(
                group.reaped,
                "the direct child must be reaped for {invalid_id}"
            );
            assert!(!group.signalled, "there was no usable group identity");
            assert_eq!(result.cleanup_failure, Some(CleanupFailure::ChildStatus));
            assert_eq!(result.output.exit_code, -1);
            assert!(!result.output.timed_out);
            assert!(result.capture.is_complete());
        }
    }

    struct ScriptedPipe {
        fd: File,
        counts: std::vec::IntoIter<usize>,
    }

    impl AsFd for ScriptedPipe {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.fd.as_fd()
        }
    }

    impl Read for ScriptedPipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            buf.fill(b'X');
            Ok(self.counts.next().unwrap_or(0))
        }
    }

    fn scripted_pipe(counts: Vec<usize>) -> PipeCapture<ScriptedPipe> {
        PipeCapture::new(
            Some(ScriptedPipe {
                fd: tempfile::tempfile().unwrap(),
                counts: counts.into_iter(),
            }),
            CaptureMode::PublishTail(2),
        )
    }

    #[test]
    #[serial_test::serial]
    fn invalid_pipe_read_closes_stream_and_marks_capture_lost() {
        let mut pipe = scripted_pipe(vec![5, READ_BYTES + 1]);
        assert!(pipe.drain_turn());
        assert!(pipe.finished());
        assert_eq!(
            pipe.buffer.finish(),
            (String::new(), Some(CaptureLoss::Read))
        );
    }

    #[test]
    #[serial_test::serial]
    fn full_chunk_and_short_followup_preserve_complete_capture() {
        let mut pipe = scripted_pipe(vec![READ_BYTES, 3]);
        assert!(pipe.drain_turn());
        assert!(pipe.finished());
        assert_eq!(pipe.buffer.finish(), ("X".repeat(READ_BYTES + 3), None));
    }
}
