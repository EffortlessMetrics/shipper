use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::state::{events, execution_state as state};
use crate::types::{
    EnvironmentFingerprint, EventType, ExecutionResult, ExecutionState, GitContext, PackageReceipt,
    PackageState, PublishEvent, Receipt, Registry, RuntimeOptions,
};
use crate::webhook::{self, WebhookEvent};

use super::super::Reporter;

pub(crate) struct ReceiptContext {
    pub(crate) plan_id: String,
    pub(crate) registry: Registry,
    pub(crate) state_dir: PathBuf,
    pub(crate) git_context: Option<GitContext>,
    pub(crate) environment: EnvironmentFingerprint,
    pub(crate) started_at: DateTime<Utc>,
}

pub(crate) fn finalize_parallel_publish(
    context: ReceiptContext,
    opts: &RuntimeOptions,
    st: &ExecutionState,
    event_log: &mut events::EventLog,
    reporter: &mut dyn Reporter,
    receipts: Vec<PackageReceipt>,
) -> Result<Receipt> {
    finalize_publish(
        context,
        opts,
        st,
        event_log,
        reporter,
        receipts,
        parallel_execution_result,
    )
}

pub(crate) fn finalize_serial_publish(
    context: ReceiptContext,
    opts: &RuntimeOptions,
    st: &ExecutionState,
    event_log: &mut events::EventLog,
    reporter: &mut dyn Reporter,
    receipts: Vec<PackageReceipt>,
) -> Result<Receipt> {
    finalize_publish(
        context,
        opts,
        st,
        event_log,
        reporter,
        receipts,
        serial_execution_result,
    )
}

fn finalize_publish(
    context: ReceiptContext,
    opts: &RuntimeOptions,
    st: &ExecutionState,
    event_log: &mut events::EventLog,
    reporter: &mut dyn Reporter,
    receipts: Vec<PackageReceipt>,
    classify_result: fn(&[PackageReceipt]) -> ExecutionResult,
) -> Result<Receipt> {
    let events_path = events::events_path(&context.state_dir);
    record_consistency_drift(&events_path, st, event_log, reporter);

    let exec_result = classify_result(&receipts);
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(&events_path)?;

    send_completion_webhook(&context.plan_id, opts, &receipts, &exec_result);

    let receipt = Receipt {
        receipt_version: "shipper.receipt.v2".to_string(),
        plan_id: context.plan_id,
        registry: context.registry,
        started_at: context.started_at,
        finished_at: Utc::now(),
        packages: receipts,
        event_log_path: context.state_dir.join("events.jsonl"),
        git_context: context.git_context,
        environment: context.environment,
    };

    state::write_receipt(&context.state_dir, &receipt)?;
    Ok(receipt)
}

fn record_consistency_drift(
    events_path: &Path,
    st: &ExecutionState,
    event_log: &mut events::EventLog,
    reporter: &mut dyn Reporter,
) {
    match crate::state::consistency::verify_events_state_consistency(events_path, st) {
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

fn send_completion_webhook(
    plan_id: &str,
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
            plan_id: plan_id.to_string(),
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

fn serial_execution_result(receipts: &[PackageReceipt]) -> ExecutionResult {
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
