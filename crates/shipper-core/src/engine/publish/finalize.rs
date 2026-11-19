use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};

use crate::engine::Reporter;
use crate::plan::PlannedWorkspace;
use crate::state::events;
use crate::state::execution_state as state;
use crate::types::{
    AuthEvidence, EnvironmentFingerprint, EventType, ExecutionResult, ExecutionState, GitContext,
    PackageReceipt, PackageState, PublishEvent, Receipt, ReleasePlan, RuntimeOptions,
};
use crate::webhook::{self, WebhookEvent};

/// Complete the evidence for a typed recoverable stop without claiming a
/// finished run or writing a receipt. Other failure classes keep their owner.
pub(in crate::engine) fn record_recoverable_stop(
    error: anyhow::Error,
    state_dir: &Path,
    events_path: &Path,
    event_log: &mut events::EventLog,
    state: &ExecutionState,
    reporter: &mut dyn Reporter,
) -> Result<anyhow::Error> {
    let Some(stop) = error.downcast_ref::<crate::engine::PublishRecoverableStopError>() else {
        return Ok(error);
    };
    let package = stop.package.clone();
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionStopped {
            reason: crate::types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
        },
        package: package.clone(),
    });
    event_log.write_to_file(events_path)?;
    record_consistency_drift(events_path, state, event_log, reporter);
    let evidence_consistent =
        crate::engine::controlled_stop_evidence_consistent(state_dir, events_path, state);
    Ok(crate::engine::PublishRecoverableStopError {
        message: format!("{error:#}"),
        package,
        evidence_consistent,
    }
    .into())
}

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
    verify_execution_receipt_scope(
        &ws.plan,
        opts.resume_from.as_deref(),
        opts.parallel.enabled,
        &receipts,
    )?;
    let exec_result = whole_plan_execution_result(&ws.plan, state);
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(events_path)?;

    let receipt = write_receipt(
        ws,
        state_dir,
        state,
        receipts,
        run_started,
        git_context,
        environment,
        auth_evidence,
        events_path,
        exec_result.clone(),
    )?;
    send_completion_webhook(ws, opts, state, &exec_result);
    Ok(receipt)
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
    verify_execution_receipt_scope(
        &ws.plan,
        opts.resume_from.as_deref(),
        opts.parallel.enabled,
        &receipts,
    )?;
    let exec_result = whole_plan_execution_result(&ws.plan, state);
    event_log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ExecutionFinished {
            result: exec_result.clone(),
        },
        package: "all".to_string(),
    });
    event_log.write_to_file(events_path)?;

    let receipt = write_receipt(
        ws,
        state_dir,
        state,
        receipts,
        run_started,
        git_context,
        environment,
        auth_evidence,
        events_path,
        exec_result.clone(),
    )?;
    send_completion_webhook(ws, opts, state, &exec_result);
    Ok(receipt)
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
    execution_result: ExecutionResult,
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
        execution_result,
    };

    crate::state::consistency::verify_plan_completion(&ws.plan, state, &receipt)?;
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

fn whole_plan_execution_result(plan: &ReleasePlan, state: &ExecutionState) -> ExecutionResult {
    if plan.packages.iter().all(|package| {
        state
            .packages
            .get(&crate::runtime::execution::pkg_key(
                &package.name,
                &package.version,
            ))
            .is_some_and(|progress| is_successful_terminal_state(&progress.state))
    }) {
        ExecutionResult::Success
    } else {
        ExecutionResult::PartialFailure
    }
}

/// A receipt covers exactly the packages admitted by this scheduler's scope.
/// Omitted prefixes keep their original state and attempt history; their
/// disposition still contributes to the whole-plan result above.
fn verify_execution_receipt_scope(
    plan: &ReleasePlan,
    resume_from: Option<&str>,
    parallel: bool,
    receipts: &[PackageReceipt],
) -> Result<()> {
    let key = |package: &crate::types::PlannedPackage| {
        crate::runtime::execution::pkg_key(&package.name, &package.version)
    };
    let expected: BTreeSet<String> = if parallel {
        let levels = plan.group_by_levels();
        let start = crate::engine::parallel::parallel_resume_start_level(&levels, resume_from)
            .ok_or_else(|| anyhow::anyhow!("validated resume target has no publish level"))?;
        levels
            .iter()
            .skip(start)
            .flat_map(|level| level.packages.iter().map(key))
            .collect()
    } else {
        plan.packages
            .iter()
            .skip_while(|package| resume_from.is_some_and(|name| package.name != name))
            .map(key)
            .collect()
    };
    let actual: BTreeSet<String> = receipts
        .iter()
        .map(|package| crate::runtime::execution::pkg_key(&package.name, &package.version))
        .collect();
    ensure!(
        actual.len() == receipts.len(),
        "duplicate package in execution receipt"
    );
    ensure!(
        actual == expected,
        "execution receipt does not cover the selected publish scope; missing: {:?}; unexpected: {:?}",
        expected.difference(&actual).collect::<Vec<_>>(),
        actual.difference(&expected).collect::<Vec<_>>(),
    );
    Ok(())
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
    state: &ExecutionState,
    exec_result: &ExecutionResult,
) {
    let total_packages = ws.plan.packages.len();
    let mut success_count = 0;
    let mut failure_count = 0;
    let mut skipped_count = 0;

    for package in &ws.plan.packages {
        let key = crate::runtime::execution::pkg_key(&package.name, &package.version);
        match state.packages.get(&key).map(|progress| &progress.state) {
            Some(PackageState::Published) => success_count += 1,
            Some(PackageState::Skipped { .. }) => skipped_count += 1,
            Some(_) | None => failure_count += 1,
        }
    }

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
    use super::*;
    use crate::types::{ErrorClass, PackageEvidence, PackageProgress, PlannedPackage, Registry};
    use std::collections::BTreeMap;

    fn plan() -> ReleasePlan {
        ReleasePlan {
            plan_version: "1".into(),
            plan_id: "whole-plan-completion".into(),
            created_at: Utc::now(),
            registry: Registry::crates_io(),
            packages: ["base", "a", "z"]
                .into_iter()
                .map(|name| PlannedPackage {
                    name: name.into(),
                    version: "0.1.0".into(),
                    manifest_path: PathBuf::from(format!("{name}/Cargo.toml")),
                    regime: None,
                })
                .collect(),
            dependencies: BTreeMap::from([
                ("a".into(), vec!["base".into()]),
                ("z".into(), vec!["base".into()]),
            ]),
        }
    }

    fn state(plan: &ReleasePlan) -> ExecutionState {
        ExecutionState {
            state_version: "shipper.state.v2".into(),
            plan_id: plan.plan_id.clone(),
            registry: plan.registry.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attempt_history: vec![],
            packages: plan
                .packages
                .iter()
                .map(|package| {
                    (
                        format!("{}@{}", package.name, package.version),
                        PackageProgress {
                            name: package.name.clone(),
                            version: package.version.clone(),
                            attempts: 0,
                            state: PackageState::Skipped {
                                reason: "already published".into(),
                            },
                            last_updated_at: Utc::now(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn receipt(name: &str) -> PackageReceipt {
        PackageReceipt {
            name: name.into(),
            version: "0.1.0".into(),
            attempts: 0,
            state: PackageState::Skipped {
                reason: "already published".into(),
            },
            started_at: Utc::now(),
            finished_at: Utc::now(),
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
    fn whole_plan_completion_requires_every_planned_package_to_be_terminal() {
        let plan = plan();
        let mut state = state(&plan);
        for prefix in [
            PackageState::Pending,
            PackageState::Uploaded,
            PackageState::Failed {
                class: ErrorClass::Retryable,
                message: "retry".into(),
            },
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "blocked".into(),
            },
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message: "unknown".into(),
            },
            PackageState::Ambiguous {
                message: "unknown".into(),
            },
        ] {
            state.packages.get_mut("base@0.1.0").expect("base").state = prefix;
            assert_eq!(
                whole_plan_execution_result(&plan, &state),
                ExecutionResult::PartialFailure
            );
        }
        for prefix in [
            PackageState::Published,
            PackageState::Skipped {
                reason: "already published".into(),
            },
        ] {
            state.packages.get_mut("base@0.1.0").expect("base").state = prefix;
            assert_eq!(
                whole_plan_execution_result(&plan, &state),
                ExecutionResult::Success
            );
        }
        state.packages.remove("base@0.1.0");
        assert_eq!(
            whole_plan_execution_result(&plan, &state),
            ExecutionResult::PartialFailure
        );
    }

    #[test]
    fn receipt_scope_matches_sequential_suffix_and_parallel_whole_level() {
        let plan = plan();
        verify_execution_receipt_scope(&plan, Some("z"), false, &[receipt("z")])
            .expect("sequential suffix");
        verify_execution_receipt_scope(&plan, Some("z"), true, &[receipt("a"), receipt("z")])
            .expect("parallel level");
        for parallel in [false, true] {
            assert!(
                verify_execution_receipt_scope(&plan, Some("z"), parallel, &[]).is_err(),
                "missing selected receipt must fail"
            );
            assert!(
                verify_execution_receipt_scope(
                    &plan,
                    Some("z"),
                    parallel,
                    &[receipt("z"), receipt("z")]
                )
                .is_err(),
                "duplicates must fail"
            );
        }
        assert!(
            verify_execution_receipt_scope(&plan, Some("z"), true, &[receipt("z")]).is_err(),
            "parallel same-level sibling cannot disappear"
        );
        assert!(
            verify_execution_receipt_scope(
                &plan,
                Some("z"),
                false,
                &[receipt("base"), receipt("z")]
            )
            .is_err(),
            "prefix must not gain a fabricated receipt"
        );
    }
}
