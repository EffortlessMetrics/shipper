//! Preserve an observed local cleanup hold through the existing event/state owner.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::cargo::CargoOutput;
use crate::ops::process::CleanupFailure;
use crate::state::{events::EventLog, execution_state};
use crate::types::{AttemptDetail, EventType, ExecutionState, PackageState, PublishEvent};

use super::PublishCleanupIncompleteError;

pub(super) fn message(failure: CleanupFailure) -> String {
    format!(
        "Cargo process cleanup is incomplete: {}; the upload outcome remains unknown; stop and inspect the owned processes before any new publication",
        failure.description()
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn stop_publish(
    shared_state: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    shared_log: &Arc<Mutex<EventLog>>,
    events_path: &Path,
    package: &str,
    message: String,
    detail: AttemptDetail,
    output: &CargoOutput,
) -> anyhow::Error {
    // Preserve the established log -> state lock ordering. Attempt admission
    // takes these same locks and checks all package holds before its event.
    let Ok(mut log) = shared_log.lock() else {
        return PublishCleanupIncompleteError {
            message: format!("{message}; cleanup evidence could not be recorded because the event log lock is poisoned; do not rerun"),
        }.into();
    };
    let Ok(mut state) = shared_state.lock() else {
        return PublishCleanupIncompleteError {
            message: format!("{message}; cleanup evidence could not be recorded because the state lock is poisoned; do not rerun"),
        }.into();
    };
    let Some(progress) = state.packages.get_mut(package) else {
        return PublishCleanupIncompleteError {
            message: format!("{message}; cleanup evidence could not be recorded because package progress is missing; do not rerun"),
        }.into();
    };
    let held = PackageState::CleanupIncomplete {
        message: message.clone(),
    };
    // This is an observed safety condition before it is a confirmed durable
    // projection. Retain it in memory even if the subsequent writes fail.
    progress.state = held.clone();
    progress.last_updated_at = detail.ended_at;
    state.updated_at = detail.ended_at;
    log.record(PublishEvent {
        timestamp: detail.ended_at,
        event_type: EventType::PackageOutput {
            stdout_tail: output.stdout_tail.clone(),
            stderr_tail: output.stderr_tail.clone(),
        },
        package: package.to_string(),
    });
    log.record(PublishEvent {
        timestamp: detail.ended_at,
        event_type: EventType::PackageCleanupIncomplete {
            message: message.clone(),
        },
        package: package.to_string(),
    });
    let persisted = super::transition::commit_pending_with_attempt_detail(
        &mut state,
        state_dir,
        &mut log,
        events_path,
        package,
        held,
        detail.clone(),
    );
    if let Err(error) = persisted {
        // A failure to append the hold must not leave a writable projection
        // looking retryable. Use the existing projection writer as a fallback;
        // this does not pretend that an authoritative event was written.
        state.attempt_history.push(detail);
        let fallback = execution_state::save_state(state_dir, &state);
        let persistence = if fallback.is_ok() {
            "the cleanup hold was saved in state, but its event/projection transition is incomplete; preserve both artifacts; an offline, exclusively invoked state rebuild must retain this hold"
        } else {
            "cleanup evidence could not be recorded reliably; no safe rerun is established; preserve the available evidence and inspect processes before any further action"
        };
        return error
            .context(PublishCleanupIncompleteError {
                message: format!("{message}; {persistence}"),
            })
            .context("failed to persist cleanup hold");
    }
    PublishCleanupIncompleteError { message }.into()
}

/// Rehearsal has no normal attempt projection. Its explicit hold event still
/// blocks repeated rehearsal and publication through the shared admission owner.
pub(super) fn stop_rehearsal(
    log: &mut EventLog,
    events_path: &Path,
    package: &str,
    message: String,
) -> anyhow::Error {
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageCleanupIncomplete {
            message: message.clone(),
        },
        package: package.to_string(),
    });
    if let Err(error) = log.write_to_file(events_path) {
        return error.context(PublishCleanupIncompleteError {
            message: format!("{message}; rehearsal cleanup evidence could not be recorded; no safe rerun is established"),
        }).context("failed to persist rehearsal cleanup hold");
    }
    log.clear();
    PublishCleanupIncompleteError { message }.into()
}
