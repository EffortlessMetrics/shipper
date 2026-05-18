use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::engine::Reporter;
use crate::plan::PlannedWorkspace;
use crate::state::events;
use crate::state::execution_state as state;
use crate::types::{
    AuthEvidence, EnvironmentFingerprint, EventType, ExecutionResult, ExecutionState, GitContext,
    PackageReceipt, PackageState, PublishEvent, Receipt, RuntimeOptions,
};
use crate::webhook::{self, WebhookEvent};

pub(in crate::engine) fn record_consistency_drift(
    events_path: &Path,
    state: &ExecutionState,
    event_log: &mut events::EventLog,
    reporter: &mut dyn Reporter,
) {
    match crate::state::consistency::verify_events_state_consistency(events_path, state) {
        Ok(drift) if !drift.is_consistent() => {
            reporter.warn(&crate::state::consistency::format_drift_summary(&drift));
            event_log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::StateEventDriftDetected { drift },
                package: "all".to_string(),
            });
        }
        Ok(_) => {}
        Err(e) => reporter.warn(&format!("end-of-run consistency check failed: {e}")),
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::engine) fn finish_sequential_run(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    events_path: &Path,
    event_log: &mut events::EventLog,
    state: &ExecutionState,
    receipts: Vec<PackageReceipt>,
    run_started: DateTime<Utc>,
    git_context: Option<GitContext>,
    environment: EnvironmentFingerprint,
    auth_evidence: AuthEvidence,
) -> Result<Receipt> {
    let exec_result = sequential_execution_result(&receipts);
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(events_path)?;

    send_completion_webhook(ws, opts, &receipts, &exec_result);

    write_receipt(
        ws,
        state_dir,
        state,
        receipts,
        run_started,
        git_context,
        environment,
        auth_evidence,
        events_path,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::engine) fn finish_parallel_run(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    events_path: &Path,
    event_log: &mut events::EventLog,
    state: &ExecutionState,
    receipts: Vec<PackageReceipt>,
    run_started: DateTime<Utc>,
    git_context: Option<GitContext>,
    environment: EnvironmentFingerprint,
    auth_evidence: AuthEvidence,
) -> Result<Receipt> {
    let exec_result = parallel_execution_result(&receipts);
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(events_path)?;

    send_completion_webhook(ws, opts, &receipts, &exec_result);

    write_receipt(
        ws,
        state_dir,
        state,
        receipts,
        run_started,
        git_context,
        environment,
        auth_evidence,
        events_path,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_receipt(
    ws: &PlannedWorkspace,
    state_dir: &Path,
    state: &ExecutionState,
    receipts: Vec<PackageReceipt>,
    run_started: DateTime<Utc>,
    git_context: Option<GitContext>,
    environment: EnvironmentFingerprint,
    auth_evidence: AuthEvidence,
    events_path: &Path,
) -> Result<Receipt> {
    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: ws.plan.plan_id.clone(),
        registry: ws.plan.registry.clone(),
        started_at: run_started,
        finished_at: Utc::now(),
        packages: receipts,
        event_log_path: PathBuf::from(state_dir).join("events.jsonl"),
        git_context,
        environment,
        auth_evidence: Some(auth_evidence),
    };

    let reconciliation_report = crate::state::reconciliation::write_report_from_events(
        state_dir,
        &ws.plan.plan_id,
        &ws.plan.registry,
        events_path,
    )?;
    crate::state::consistency::verify_finalization_consistency(
        events_path,
        state,
        &receipt,
        reconciliation_report.as_ref(),
    )?;
    state::write_receipt(state_dir, &receipt)?;
    Ok(receipt)
}

fn sequential_execution_result(receipts: &[PackageReceipt]) -> ExecutionResult {
    if receipts
        .iter()
        .all(|r| is_successful_terminal_state(&r.state))
    {
        ExecutionResult::Success
    } else {
        ExecutionResult::PartialFailure
    }
}

fn parallel_execution_result(receipts: &[PackageReceipt]) -> ExecutionResult {
    if receipts
        .iter()
        .all(|r| is_successful_terminal_state(&r.state))
    {
        ExecutionResult::Success
    } else {
        ExecutionResult::PartialFailure
    }
}

fn is_successful_terminal_state(state: &PackageState) -> bool {
    matches!(
        state,
        PackageState::Published | PackageState::Skipped { .. }
    )
}

fn send_completion_webhook(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    receipts: &[PackageReceipt],
    exec_result: &ExecutionResult,
) {
    let total_packages = receipts.len();
    let success_count = receipts
        .iter()
        .filter(|r| matches!(r.state, PackageState::Published))
        .count();
    let failure_count = receipts
        .iter()
        .filter(|r| !is_successful_terminal_state(&r.state))
        .count();
    let skipped_count = receipts
        .iter()
        .filter(|r| matches!(r.state, PackageState::Skipped { .. }))
        .count();

    webhook::maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishCompleted {
            plan_id: ws.plan.plan_id.clone(),
            total_packages,
            success_count,
            failure_count,
            skipped_count,
            result: match exec_result {
                ExecutionResult::Success => "success".to_string(),
                ExecutionResult::PartialFailure => "partial_failure".to_string(),
                ExecutionResult::CompleteFailure => "complete_failure".to_string(),
            },
        },
    );
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        is_successful_terminal_state, parallel_execution_result, sequential_execution_result,
    };
    use crate::types::{ExecutionResult, PackageEvidence, PackageReceipt, PackageState};

    fn receipt(state: PackageState) -> PackageReceipt {
        let now = Utc::now();
        PackageReceipt {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state,
            started_at: now,
            finished_at: now,
            duration_ms: 0,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        }
    }

    #[test]
    fn sequential_uploaded_receipt_is_not_terminal_success() {
        let receipts = [receipt(PackageState::Uploaded)];

        assert_eq!(
            sequential_execution_result(&receipts),
            ExecutionResult::PartialFailure
        );
    }

    #[test]
    fn parallel_uploaded_receipt_is_not_terminal_success() {
        let receipts = [receipt(PackageState::Uploaded)];

        assert_eq!(
            parallel_execution_result(&receipts),
            ExecutionResult::PartialFailure
        );
    }

    #[test]
    fn completion_metrics_count_uploaded_as_failure() {
        let receipts = [receipt(PackageState::Uploaded)];
        let failure_count = receipts
            .iter()
            .filter(|receipt| !is_successful_terminal_state(&receipt.state))
            .count();

        assert_eq!(failure_count, 1);
    }
}
