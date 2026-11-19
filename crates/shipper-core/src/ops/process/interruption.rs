//! Explicit process-lifetime cancellation state; OS signal policy belongs to the CLI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use anyhow::anyhow;
use anyhow::{Result, ensure};

struct Context {
    requested: Arc<AtomicBool>,
    default_action: Arc<AtomicBool>,
    leases: Mutex<usize>,
}

static CONTEXT: OnceLock<Context> = OnceLock::new();

/// Unsupported bridge for the separate CLI process entry point.
///
/// This installs no OS handlers. The CLI explicitly registers process-lifetime
/// handlers using these flags before running any commands. A request is sticky;
/// neither flag may be reset/replaced by callers. Normal embedding entry points
/// do not initialize this context and retain their host's signal policy.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct PublishInterruption(&'static Context);

impl PublishInterruption {
    pub fn process_wide() -> Self {
        Self(CONTEXT.get_or_init(|| Context {
            requested: Arc::new(AtomicBool::new(false)),
            default_action: Arc::new(AtomicBool::new(true)),
            leases: Mutex::new(0),
        }))
    }

    /// Register this flag after the conditional-default action for each signal.
    pub fn request_flag(self) -> Arc<AtomicBool> {
        Arc::clone(&self.0.requested)
    }

    /// Permit ordinary termination outside supervision and outcome persistence.
    pub fn default_action_flag(self) -> Arc<AtomicBool> {
        Arc::clone(&self.0.default_action)
    }
}

pub(crate) fn requested() -> bool {
    CONTEXT
        .get()
        .is_some_and(|context| context.requested.load(Ordering::SeqCst))
}

#[derive(Debug, thiserror::Error)]
#[error("publication interrupted; no further command may start in this process")]
pub(crate) struct Interrupted;

pub(crate) fn check() -> Result<()> {
    ensure!(!requested(), Interrupted);
    Ok(())
}

/// Keep termination deferred until an owned child is cleaned and its uncertain
/// outcome has been recorded. No handler ever locks this mutex.
pub(crate) struct InterruptionLease(&'static Context);

impl InterruptionLease {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    pub(crate) fn acquire() -> Result<Option<Self>> {
        let Some(context) = CONTEXT.get() else {
            return Ok(None);
        };
        let mut leases = context
            .leases
            .lock()
            .map_err(|_| anyhow!("publish interruption lease lock is poisoned"))?;
        *leases = leases
            .checked_add(1)
            .ok_or_else(|| anyhow!("too many concurrent publish interruption leases"))?;
        context.default_action.store(false, Ordering::SeqCst);
        Ok(Some(Self(context)))
    }
}

impl Drop for InterruptionLease {
    fn drop(&mut self) {
        let mut leases = self
            .0
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *leases -= 1;
        if *leases == 0 {
            self.0.default_action.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const PROBE: &str = "ops::process::interruption::tests::interruption_process_probe";

    #[test]
    #[serial_test::serial]
    #[ignore = "process-lifetime interruption state is tested in an isolated subprocess"]
    fn interruption_process_probe() {
        let context = PublishInterruption::process_wide();
        let defaults = context.default_action_flag();
        assert!(defaults.load(Ordering::SeqCst));
        let first = InterruptionLease::acquire().unwrap();
        let second = InterruptionLease::acquire().unwrap();
        assert!(!defaults.load(Ordering::SeqCst));
        drop(first);
        assert!(!defaults.load(Ordering::SeqCst));
        drop(second);
        assert!(defaults.load(Ordering::SeqCst));

        let directory = tempfile::tempdir().unwrap();
        let completed = crate::ops::process::run_command_with_publish_capture(
            "sh",
            &["-c", "echo completed"],
            directory.path(),
            None,
            10,
        )
        .unwrap();
        assert!(!completed.interrupted);
        assert_eq!(completed.cleanup_failure, None);
        assert!(
            !defaults.load(Ordering::SeqCst),
            "normal results retain their lease through the engine handoff"
        );
        let ready = directory.path().join("ready");
        let flag = context.request_flag();
        let waiter = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !ready.exists() {
                assert!(Instant::now() < deadline, "child startup did not happen");
                std::thread::sleep(Duration::from_millis(5));
            }
            flag.store(true, Ordering::SeqCst);
        });
        let output = crate::ops::process::run_command_with_publish_capture(
            "sh",
            &[
                "-c",
                "echo $$ > child-pid; echo sentinel; touch ready; exec sleep 30",
            ],
            directory.path(),
            None,
            10,
        )
        .unwrap();
        waiter.join().unwrap();
        assert!(output.interrupted);
        assert_eq!(output.cleanup_failure, None);
        assert!(output.output.stdout.contains("sentinel"));
        assert!(output.output.duration < Duration::from_secs(3));
        assert!(
            !defaults.load(Ordering::SeqCst),
            "uncertain outcome retains the lease"
        );
        let error = crate::ops::process::run_command_with_publish_capture(
            "sh",
            &["-c", "touch should-not-exist"],
            directory.path(),
            None,
            10,
        )
        .err()
        .expect("a latched interrupt prevents a later spawn");
        assert!(error.downcast_ref::<Interrupted>().is_some());
        assert!(!directory.path().join("should-not-exist").exists());
        assert!(
            !defaults.load(Ordering::SeqCst),
            "refused nested call cannot release another lease"
        );
        drop(output);
        assert!(
            !defaults.load(Ordering::SeqCst),
            "a peer's normal result still owns its outcome decision"
        );
        drop(completed);
        assert!(defaults.load(Ordering::SeqCst));
        assert!(
            requested(),
            "finishing supervision never resets a shutdown request"
        );
    }

    #[test]
    #[serial_test::serial]
    fn interruption_leases_retain_persistence_and_refuse_later_spawns() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", PROBE, "--ignored", "--nocapture"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!(
                    "interruption witness exceeded its watchdog: {:?}",
                    child.wait_with_output().unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(
            CONTEXT.get().is_none(),
            "ordinary core callers never install cancellation state"
        );
    }
}
