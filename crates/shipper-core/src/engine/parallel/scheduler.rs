//! Dependency-level scheduling for the canonical package executor.

use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Result, bail};

use crate::engine::execute_package::{PackagePublishResult, publish_package};
use crate::plan::PlannedWorkspace;
use crate::plan::chunking::chunk_by_max_concurrent;
use crate::registry::RegistryClient;
use crate::state::events;
use shipper_types::{ExecutionState, PackageReceipt, PublishLevel, RuntimeOptions};

use super::{Reporter, SendReporter, replay_buffered_messages};

struct WorkerHandle {
    handle: thread::JoinHandle<PackagePublishResult>,
    #[cfg(test)]
    join_should_fail: bool,
}

fn finish_publish_errors(mut errors: Vec<anyhow::Error>) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    let summary = errors
        .iter()
        .map(|error| format!("{error:#}"))
        .collect::<Vec<_>>()
        .join("; ");
    if let Some(index) = errors.iter().position(|error| {
        error
            .downcast_ref::<crate::engine::PublishCleanupIncompleteError>()
            .is_some()
    }) {
        let cleanup = errors.swap_remove(index);
        return Err(cleanup.context(format!(
            "parallel publish stopped after incomplete process cleanup: {summary}"
        )));
    }
    if let Some(index) = errors.iter().position(|error| {
        error
            .downcast_ref::<crate::ops::process::interruption::Interrupted>()
            .is_some()
    }) {
        return Err(errors.swap_remove(index).context(format!(
            "parallel publication interrupted after joining active workers: {summary}"
        )));
    }
    if let Some(index) = errors.iter().position(|error| {
        error
            .downcast_ref::<crate::engine::PublishStillUnknownError>()
            .is_some()
    }) {
        let still_unknown = errors.swap_remove(index);
        return Err(still_unknown.context(format!(
            "parallel publish failed for {} package(s): {summary}",
            errors.len() + 1
        )));
    }
    if let Some(index) = errors.iter().position(|error| {
        error
            .downcast_ref::<crate::engine::PublishRecoverableStopError>()
            .is_none()
    }) {
        let blocker = errors.swap_remove(index);
        return Err(blocker.context(format!(
            "parallel publish failed for {} package(s): {summary}",
            errors.len() + 1
        )));
    }
    let package = errors
        .iter()
        .find_map(|error| {
            error
                .downcast_ref::<crate::engine::PublishRecoverableStopError>()
                .map(|stop| stop.package.clone())
        })
        .unwrap_or_else(|| "workspace".to_string());
    Err(crate::engine::PublishRecoverableStopError {
        message: format!(
            "parallel publish stopped recoverably for {} package(s): {summary}",
            errors.len()
        ),
        package,
        // A worker-local bit can never authorize the run. Only run_publish's
        // post-join, post-marker full consistency predicate may promote this.
        evidence_consistent: false,
    }
    .into())
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
    let mut errors: Vec<anyhow::Error> = Vec::new();

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
            send_reporter.wait_for_messages();
            replay_buffered_messages(reporter, send_reporter.as_ref());
        }
        replay_buffered_messages(reporter, send_reporter.as_ref());

        let mut join_failures = 0;
        for handle in handles {
            match handle.join() {
                Ok(result) => match result.result {
                    Ok(receipt) => all_receipts.push(receipt),
                    Err(error) => errors.push(error),
                },
                Err(_) => join_failures += 1,
            }
        }

        let cleanup_incomplete = errors.iter().any(|error| {
            error
                .downcast_ref::<crate::engine::PublishCleanupIncompleteError>()
                .is_some()
        });
        if join_failures > 0 && cleanup_incomplete {
            errors.push(anyhow::anyhow!(
                "parallel publish worker join failed for {join_failures} package(s)"
            ));
        } else if join_failures > 0 {
            bail!("parallel publish worker join failed for {join_failures} package(s)");
        }
        if cleanup_incomplete || crate::ops::process::interruption::requested() {
            // All already-admitted peers have been joined. Never launch a
            // later chunk after an owned process may still be running.
            if !cleanup_incomplete {
                errors.push(crate::ops::process::interruption::Interrupted.into());
            }
            return finish_publish_errors(errors).map(|()| all_receipts);
        }
    }

    finish_publish_errors(errors)?;
    Ok(all_receipts)
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow, bail, ensure};

    use super::finish_publish_errors;

    #[test]
    fn cleanup_hold_identity_outranks_unknown_and_join_failure() {
        let error = finish_publish_errors(vec![
            anyhow!("parallel publish worker join failed"),
            crate::engine::PublishStillUnknownError {
                message: "registry unknown".into(),
                reconciliation_written: true,
            }
            .into(),
            crate::engine::PublishCleanupIncompleteError {
                message: "owned cleanup incomplete".into(),
            }
            .into(),
        ])
        .unwrap_err();
        assert!(matches!(
            crate::engine::classify_publish_stop(&error),
            Some(crate::engine::PublishStopClassification::CleanupIncomplete)
        ));
        assert!(format!("{error:#}").contains("worker join failed"));
    }

    #[test]
    fn still_unknown_identity_survives_parallel_error_aggregation() -> Result<()> {
        let errors = vec![
            anyhow!("unrelated worker failure"),
            crate::engine::PublishStillUnknownError {
                message: "demo@0.1.0: reconciliation inconclusive".to_string(),
                reconciliation_written: true,
            }
            .into(),
        ];
        let Err(error) = finish_publish_errors(errors) else {
            bail!("parallel aggregation unexpectedly succeeded");
        };
        ensure!(
            error
                .downcast_ref::<crate::engine::PublishStillUnknownError>()
                .is_some(),
            "typed StillUnknown identity was lost: {error:#}"
        );
        ensure!(format!("{error:#}").contains("unrelated worker failure"));
        Ok(())
    }

    #[test]
    fn all_recoverable_aggregation_erases_mixed_worker_authorization() -> Result<()> {
        let errors = vec![
            crate::engine::PublishRecoverableStopError {
                message: "a stopped recoverably".to_string(),
                package: "a@1.0.0".to_string(),
                evidence_consistent: true,
            }
            .into(),
            crate::engine::PublishRecoverableStopError {
                message: "b stopped recoverably".to_string(),
                package: "b@1.0.0".to_string(),
                evidence_consistent: false,
            }
            .into(),
        ];
        let Err(error) = finish_publish_errors(errors) else {
            bail!("parallel aggregation unexpectedly succeeded");
        };
        ensure!(
            matches!(
                crate::engine::classify_publish_stop(&error),
                Some(
                    crate::engine::PublishStopClassification::RecoverableNotPublished {
                        evidence_consistent: false
                    }
                )
            ),
            "typed recoverable stop identity was lost: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn ordinary_blocker_outranks_parallel_recoverable_stop() -> Result<()> {
        let errors = vec![
            crate::engine::PublishRecoverableStopError {
                message: "a stopped recoverably".to_string(),
                package: "a@1.0.0".to_string(),
                evidence_consistent: false,
            }
            .into(),
            anyhow!("permanent worker blocker"),
        ];
        let Err(error) = finish_publish_errors(errors) else {
            bail!("parallel aggregation unexpectedly succeeded");
        };
        ensure!(
            error
                .downcast_ref::<crate::engine::PublishRecoverableStopError>()
                .is_none(),
            "recoverable identity incorrectly outranked blocker: {error:#}"
        );
        ensure!(format!("{error:#}").contains("permanent worker blocker"));
        Ok(())
    }
}
