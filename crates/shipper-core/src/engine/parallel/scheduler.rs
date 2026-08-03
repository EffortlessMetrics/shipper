//! Dependency-level scheduling for the canonical package executor.

use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::engine::execute_package::{PackagePublishResult, publish_package};
use crate::plan::PlannedWorkspace;
use crate::plan::chunking::chunk_by_max_concurrent;
use crate::registry::RegistryClient;
use crate::state::events;
use shipper_types::{ExecutionState, PackageReceipt, PublishLevel, RuntimeOptions};

use super::{Reporter, SendReporter, drain_retry_waits};

struct WorkerHandle {
    handle: thread::JoinHandle<PackagePublishResult>,
    #[cfg(test)]
    join_should_fail: bool,
}

impl WorkerHandle {
    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    fn join(self) -> thread::Result<PackagePublishResult> {
        let result = self.handle.join();
        #[cfg(test)]
        if self.join_should_fail {
            return Err(Box::new("injected worker join failure"));
        }
        result
    }
}

#[cfg(test)]
static INJECT_WORKER_JOIN_FAILURE: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) struct WorkerJoinFailureGuard;

#[cfg(test)]
impl Drop for WorkerJoinFailureGuard {
    fn drop(&mut self) {
        INJECT_WORKER_JOIN_FAILURE.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
pub(crate) fn inject_worker_join_failure() -> WorkerJoinFailureGuard {
    INJECT_WORKER_JOIN_FAILURE.store(true, Ordering::SeqCst);
    WorkerJoinFailureGuard
}

#[cfg(test)]
fn take_worker_join_failure_injection() -> bool {
    INJECT_WORKER_JOIN_FAILURE
        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// Publish packages in one dependency level using the canonical executor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_publish_level(
    level: &PublishLevel,
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    reporter: &mut dyn Reporter,
    send_reporter: &Arc<SendReporter>,
) -> Result<Vec<PackageReceipt>> {
    let num_packages = level.packages.len();
    let max_concurrent = opts.parallel.max_concurrent.min(num_packages);

    reporter.info(&format!(
        "Level {}: publishing {} packages (max concurrent: {})",
        level.level, num_packages, max_concurrent
    ));

    let mut all_receipts: Vec<PackageReceipt> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for chunk in chunk_by_max_concurrent(&level.packages, max_concurrent) {
        let mut handles: Vec<WorkerHandle> = Vec::new();

        for p in chunk {
            let p = p.clone();
            let ws_clone = ws.clone();
            let opts_clone = opts.clone();
            let reg_clone = reg.clone();
            let st_clone = Arc::clone(st);
            let state_dir = state_dir.to_path_buf();
            let event_log_clone = Arc::clone(event_log);
            let events_path = events_path.to_path_buf();
            let reporter_clone = Arc::clone(send_reporter);

            let handle = thread::spawn(move || {
                publish_package(
                    &p,
                    &ws_clone,
                    &opts_clone,
                    &reg_clone,
                    &st_clone,
                    &state_dir,
                    &event_log_clone,
                    &events_path,
                    &reporter_clone,
                )
            });

            handles.push(WorkerHandle {
                handle,
                #[cfg(test)]
                join_should_fail: take_worker_join_failure_injection(),
            });
        }

        while handles.iter().any(|handle| !handle.is_finished()) {
            drain_retry_waits(reporter, send_reporter.as_ref());
            thread::sleep(Duration::from_millis(25));
        }
        drain_retry_waits(reporter, send_reporter.as_ref());

        let mut join_failures = 0;
        for handle in handles {
            match handle.join() {
                Ok(result) => match result.result {
                    Ok(receipt) => all_receipts.push(receipt),
                    Err(e) => errors.push(format!("{e:#}")),
                },
                Err(_) => join_failures += 1,
            }
        }

        if join_failures > 0 {
            bail!("parallel publish worker join failed for {join_failures} package(s)");
        }
    }

    if !errors.is_empty() {
        bail!(
            "parallel publish failed for {} package(s): {}",
            errors.len(),
            errors.join("; ")
        );
    }

    Ok(all_receipts)
}
