use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::engine::Reporter;
use crate::plan::PlannedWorkspace;
use crate::state::events;
use crate::state::execution_state as state;
use crate::types::{
    EnvironmentFingerprint, EventType, ExecutionResult, ExecutionState, GitContext, PackageReceipt,
    PackageState, PublishEvent, Receipt, RuntimeOptions,
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
    receipts: Vec<PackageReceipt>,
    run_started: DateTime<Utc>,
    git_context: Option<GitContext>,
    environment: EnvironmentFingerprint,
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
        receipts,
        run_started,
        git_context,
        environment,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::engine) fn finish_parallel_run(
    ws: &PlannedWorkspace,
    state_dir: &Path,
    events_path: &Path,
    event_log: &mut events::EventLog,
    receipts: Vec<PackageReceipt>,
    run_started: DateTime<Utc>,
    git_context: Option<GitContext>,
    environment: EnvironmentFingerprint,
) -> Result<Receipt> {
    let exec_result = parallel_execution_result(&receipts);
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result,
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(events_path)?;

    write_receipt(
        ws,
        state_dir,
        receipts,
        run_started,
        git_context,
        environment,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_receipt(
    ws: &PlannedWorkspace,
    state_dir: &Path,
    receipts: Vec<PackageReceipt>,
    run_started: DateTime<Utc>,
    git_context: Option<GitContext>,
    environment: EnvironmentFingerprint,
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
    };

    state::write_receipt(state_dir, &receipt)?;
    Ok(receipt)
}

fn sequential_execution_result(receipts: &[PackageReceipt]) -> ExecutionResult {
    if receipts.iter().all(|r| {
        matches!(
            r.state,
            PackageState::Published | PackageState::Uploaded | PackageState::Skipped { .. }
        )
    }) {
        ExecutionResult::Success
    } else {
        ExecutionResult::PartialFailure
    }
}

fn parallel_execution_result(receipts: &[PackageReceipt]) -> ExecutionResult {
    if receipts.iter().all(|r| {
        matches!(
            r.state,
            PackageState::Published | PackageState::Skipped { .. }
        )
    }) {
        ExecutionResult::Success
    } else {
        ExecutionResult::PartialFailure
    }
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
        .filter(|r| matches!(r.state, PackageState::Failed { .. }))
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
