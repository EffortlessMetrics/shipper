//! End-of-run consistency check between `events.jsonl` and `state.json`.
//!
//! **Layer:** state (layer 3).
//!
//! Per [docs/INVARIANTS.md](https://github.com/EffortlessMetrics/shipper/blob/main/docs/INVARIANTS.md),
//! `events.jsonl` is the authoritative truth and `state.json` is a projection.
//! They must agree on which packages were published. This module surfaces
//! any drift loudly at the end of a run so an operator (or auditor) doesn't
//! silently trust a stale or corrupted projection.
//!
//! See [issue #93](https://github.com/EffortlessMetrics/shipper/issues/93).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};

use shipper_types::{
    ErrorClass, EventType, ExecutionResult, ExecutionState, PackageState, Receipt,
    ReconciliationOutcome, ReconciliationReport, ReleasePlan, StateEventDrift,
};

use super::events::EventLog;
use super::rebuild::{StateRebuildOptions, rebuild_state_from_events};

/// Verify that `events.jsonl` and the in-memory `ExecutionState` agree on which
/// packages are Published.
///
/// Reads the event log from disk and compares the set of packages with a
/// `PackagePublished` event against the set of packages whose current state
/// is `PackageState::Published`. Returns a [`StateEventDrift`] describing any
/// mismatch; use [`StateEventDrift::is_consistent`] to branch.
///
/// Only an I/O failure reading the event log surfaces as `Err`; a disagreement
/// is a legitimate result of the check and returned as `Ok(drift)`.
pub fn verify_events_state_consistency(
    events_path: &Path,
    state: &ExecutionState,
) -> Result<StateEventDrift> {
    let log = EventLog::read_from_file(events_path).with_context(|| {
        format!(
            "failed to read event log for consistency check: {}",
            events_path.display()
        )
    })?;

    // Package labels that have a `PackagePublished` event in events.jsonl, or a
    // trusted resume skip event documenting a previously published package.
    // Labels are the `package` field of the event (format: `name@version`).
    let events_published: BTreeSet<String> = log
        .all_events()
        .iter()
        .filter(|e| match &e.event_type {
            EventType::PackagePublished { .. } => true,
            EventType::PackageSkipped { reason } => reason == "resume: state already published",
            _ => false,
        })
        .map(|e| e.package.clone())
        .collect();

    // Package labels that are currently marked `Published` in state.json.
    // The `packages` map is keyed by `name@version`.
    let state_published: BTreeSet<String> = state
        .packages
        .iter()
        .filter(|(_, pr)| matches!(pr.state, PackageState::Published))
        .map(|(k, _)| k.clone())
        .collect();

    let in_events_only: Vec<String> = events_published
        .difference(&state_published)
        .cloned()
        .collect();
    let in_state_only: Vec<String> = state_published
        .difference(&events_published)
        .cloned()
        .collect();

    Ok(StateEventDrift {
        in_events_only,
        in_state_only,
    })
}

/// Verify a nonterminal state and optional reconciliation report against the
/// full authoritative event projection without requiring a completed receipt.
pub(crate) fn verify_unfinished_consistency(
    events_path: &Path,
    state: &ExecutionState,
    reconciliation_report: Option<&ReconciliationReport>,
) -> Result<()> {
    let event_log = EventLog::read_from_file(events_path).with_context(|| {
        format!(
            "failed to read event log for unfinished consistency check: {}",
            events_path.display()
        )
    })?;
    let rebuilt_state = rebuild_state_from_events(
        events_path,
        StateRebuildOptions::new(state.registry.clone()).with_fallback_plan_id(&state.plan_id),
    )?;
    let mut findings = Vec::new();
    if rebuilt_state.plan_id != state.plan_id {
        findings.push(format!(
            "events plan_id {} does not match state plan_id {}",
            rebuilt_state.plan_id, state.plan_id
        ));
    }
    verify_state_matches_events(
        state,
        &rebuilt_state,
        &trusted_resume_terminal_skips(&event_log),
        legacy_attempt_metadata(&event_log),
        &mut findings,
    );
    verify_reconciliation_evidence(
        &event_log,
        &state.plan_id,
        reconciliation_report,
        &mut findings,
    );
    if findings.is_empty() {
        Ok(())
    } else {
        bail!(
            "unfinished release evidence drift detected\n{}",
            findings
                .into_iter()
                .map(|finding| format!("  - {finding}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

/// Verify the exact evidence tuple that authorizes a controlled retry after a
/// conclusive NotPublished reconciliation. A reconciliation label alone is
/// deliberately insufficient.
pub(crate) fn verify_controlled_stop_consistency(
    events_path: &Path,
    state: &ExecutionState,
    reconciliation_report: Option<&ReconciliationReport>,
) -> Result<()> {
    if let Some(report) = reconciliation_report {
        verify_reconciliation_record_identities(report)?;
    }
    verify_unfinished_consistency(events_path, state, reconciliation_report)?;
    let event_log = EventLog::read_from_file(events_path)?;
    let Some(last) = event_log.all_events().last() else {
        bail!("controlled stop requires an authoritative terminal marker");
    };
    let EventType::ExecutionStopped {
        reason: shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
    } = &last.event_type
    else {
        bail!("authoritative event tail is not a controlled NotPublished stop");
    };
    let Some(progress) = state.packages.get(&last.package) else {
        bail!("controlled-stop package is absent from state");
    };
    if !matches!(
        progress.state,
        PackageState::Failed {
            class: ErrorClass::Retryable,
            ..
        }
    ) {
        bail!("controlled-stop package is not retryable in state");
    }
    let Some(report) = reconciliation_report else {
        bail!("controlled NotPublished stop requires reconciliation evidence");
    };
    if report.plan_id != state.plan_id
        || report.registry.name != state.registry.name
        || report.registry.api_base != state.registry.api_base
        || report.registry.index_base != state.registry.index_base
    {
        bail!("controlled-stop reconciliation identity disagrees with state");
    }
    if has_latest_still_unknown(report) {
        bail!("controlled stop retains a latest StillUnknown reconciliation outcome");
    }
    let latest_retry_allowed = |package: &str| latest_retry_allowed(report, package);
    if !latest_retry_allowed(&last.package) {
        bail!("controlled-stop package lacks latest NotPublished retry evidence");
    }
    for (package, package_progress) in &state.packages {
        match &package_progress.state {
            PackageState::Published | PackageState::Skipped { .. } | PackageState::Pending => {}
            PackageState::Failed {
                class: ErrorClass::Retryable,
                ..
            } if latest_retry_allowed(package) => {}
            PackageState::Failed {
                class: ErrorClass::Retryable,
                ..
            } => bail!("retryable package {package} lacks latest NotPublished retry evidence"),
            _ => bail!(
                "controlled stop contains a package posture that is not safely resumable: {package}"
            ),
        }
    }
    Ok(())
}

fn latest_retry_allowed(report: &ReconciliationReport, package: &str) -> bool {
    report
        .records
        .iter()
        .rev()
        .find(|record| record.package == package)
        .is_some_and(|record| {
            matches!(record.outcome, ReconciliationOutcome::NotPublished { .. })
                && matches!(
                    record.operator_action,
                    shipper_types::ReconciliationOperatorAction::RetryAllowed
                )
        })
}

fn verify_reconciliation_record_identities(report: &ReconciliationReport) -> Result<()> {
    for record in &report.records {
        let expected_package = format!("{}@{}", record.name, record.version);
        if record.package != expected_package {
            bail!(
                "reconciliation package identity disagrees: package {} does not match {}",
                record.package,
                expected_package
            );
        }
    }
    Ok(())
}

fn has_latest_still_unknown(report: &ReconciliationReport) -> bool {
    let mut seen = BTreeSet::new();
    report.records.iter().rev().any(|record| {
        seen.insert(&record.package)
            && matches!(record.outcome, ReconciliationOutcome::StillUnknown { .. })
    })
}

pub(crate) fn has_not_published_retryable_posture(
    state: &ExecutionState,
    reconciliation_report: Option<&ReconciliationReport>,
) -> bool {
    let Some(report) = reconciliation_report else {
        return false;
    };
    if verify_reconciliation_record_identities(report).is_err() {
        return false;
    }
    let mut saw_retryable = false;
    for (package, progress) in &state.packages {
        match &progress.state {
            PackageState::Published | PackageState::Skipped { .. } | PackageState::Pending => {}
            PackageState::Failed {
                class: ErrorClass::Retryable,
                ..
            } if latest_retry_allowed(report, package) => saw_retryable = true,
            _ => return false,
        }
    }
    saw_retryable
}

/// Recognize a possible controlled stop independently of derived state/report
/// identities. This only selects the full consistency validator; it never
/// authorizes a retry or manufactures reconciliation evidence.
///
/// The pair must belong to the same package's latest unresolved attempt.
/// Informational run/package starts preserve it; actual attempts and outcomes
/// supersede it, and a finished execution retires history. Other unresolved
/// failure or upload postures retain their existing recovery owner, matching
/// parallel aggregation where ordinary blockers and StillUnknown outrank this
/// stop.
pub(crate) fn has_current_not_published_retryable_failure(events_path: &Path) -> Result<bool> {
    #[derive(Clone, Copy)]
    enum Posture {
        ReconciledAbsent,
        RetryableAbsent,
        OtherRecovery,
    }

    let log = EventLog::read_from_file(events_path)?;
    let mut packages = BTreeMap::new();
    for event in log.all_events() {
        let key = event.package.as_str();
        match &event.event_type {
            EventType::ExecutionFinished { .. } => packages.clear(),
            EventType::PublishReconciled {
                outcome: ReconciliationOutcome::NotPublished { .. },
            } => {
                packages.insert(key, Posture::ReconciledAbsent);
            }
            EventType::PackageFailed {
                class: ErrorClass::Retryable,
                ..
            } => {
                let posture = match packages.get(key) {
                    Some(Posture::ReconciledAbsent | Posture::RetryableAbsent) => {
                        Posture::RetryableAbsent
                    }
                    _ => Posture::OtherRecovery,
                };
                packages.insert(key, posture);
            }
            EventType::PackageAttempted { .. }
            | EventType::PackageUploaded
            | EventType::PackageFailed { .. }
            | EventType::PublishReconciling { .. }
            | EventType::PublishReconciled {
                outcome: ReconciliationOutcome::StillUnknown { .. },
            } => {
                packages.insert(key, Posture::OtherRecovery);
            }
            EventType::PackagePublished { .. }
            | EventType::PackageSkipped { .. }
            | EventType::PublishReconciled {
                outcome: ReconciliationOutcome::Published { .. },
            } => {
                packages.remove(key);
            }
            _ => {}
        }
    }
    Ok(packages
        .values()
        .any(|posture| matches!(posture, Posture::RetryableAbsent))
        && !packages
            .values()
            .any(|posture| matches!(posture, Posture::OtherRecovery)))
}

#[cfg(test)]
mod controlled_candidate_tests;

/// Render a human-readable summary of a drift report. Used by the Reporter
/// to surface the finding loudly at end of run.
pub fn format_drift_summary(drift: &StateEventDrift) -> String {
    drift_summary::format(drift)
}

mod drift_summary {
    use shipper_types::StateEventDrift;

    const CONSISTENT_MESSAGE: &str = "events.jsonl and state.json are consistent";
    const DRIFT_HEADER: &str = "state/event drift detected (events.jsonl is authoritative):";

    pub(super) fn format(drift: &StateEventDrift) -> String {
        if drift.is_consistent() {
            return CONSISTENT_MESSAGE.to_string();
        }

        let mut lines = vec![DRIFT_HEADER.to_string()];
        push_events_only_line(&mut lines, drift);
        push_state_only_line(&mut lines, drift);
        lines.join("\n")
    }

    fn push_events_only_line(lines: &mut Vec<String>, drift: &StateEventDrift) {
        if drift.in_events_only.is_empty() {
            return;
        }

        lines.push(format_package_line(
            "published in events.jsonl but NOT in state.json",
            &drift.in_events_only,
        ));
    }

    fn push_state_only_line(lines: &mut Vec<String>, drift: &StateEventDrift) {
        if drift.in_state_only.is_empty() {
            return;
        }

        lines.push(format_package_line(
            "marked published in state.json but NO event",
            &drift.in_state_only,
        ));
    }

    fn format_package_line(prefix: &str, packages: &[String]) -> String {
        format!("  {prefix} ({}): {}", packages.len(), packages.join(", "))
    }
}

/// Check completion against the entire current plan, including packages omitted
/// from a scoped invocation's receipt. Missing state is missing evidence, and
/// nonterminal state can never support a whole-plan success claim.
pub fn verify_plan_completion(
    plan: &ReleasePlan,
    state: &ExecutionState,
    receipt: &Receipt,
) -> Result<()> {
    if !plan.packages.is_empty() && receipt.packages.is_empty() {
        bail!("nonempty release plan has an empty execution receipt");
    }
    let planned: BTreeSet<String> = plan
        .packages
        .iter()
        .map(|package| format!("{}@{}", package.name, package.version))
        .collect();
    for key in &planned {
        let progress = state
            .packages
            .get(key)
            .with_context(|| format!("{key}: planned package has no durable state"))?;
        if format!("{}@{}", progress.name, progress.version) != *key {
            bail!("{key}: durable package identity does not match its planned key");
        }
        if receipt.execution_result == ExecutionResult::Success
            && !matches!(
                progress.state,
                PackageState::Published | PackageState::Skipped { .. }
            )
        {
            bail!(
                "{key}: whole-plan success conflicts with {} durable state",
                state_name(&progress.state)
            );
        }
    }
    let mut receipted = BTreeSet::new();
    for package in &receipt.packages {
        let key = format!("{}@{}", package.name, package.version);
        if !planned.contains(&key) || !receipted.insert(key.clone()) {
            bail!("{key}: receipt package is duplicated or outside the current plan");
        }
    }
    Ok(())
}

/// Verify the end-of-run evidence packet before `receipt.json` becomes the
/// durable summary.
///
/// This check is stricter than [`verify_events_state_consistency`]: it compares
/// the current state projection, the receipt that is about to be written, the
/// event-derived state projection, and reconciliation evidence when
/// reconciliation events exist. Drift is returned as an error so finalization
/// cannot produce a misleading receipt.
pub fn verify_finalization_consistency(
    events_path: &Path,
    state: &ExecutionState,
    receipt: &Receipt,
    reconciliation_report: Option<&ReconciliationReport>,
) -> Result<()> {
    let event_log = EventLog::read_from_file(events_path).with_context(|| {
        format!(
            "failed to read event log for finalization consistency check: {}",
            events_path.display()
        )
    })?;
    let rebuilt_state = rebuild_state_from_events(
        events_path,
        StateRebuildOptions::new(receipt.registry.clone()).with_fallback_plan_id(&receipt.plan_id),
    )
    .with_context(|| {
        format!(
            "failed to rebuild state projection from events for finalization check: {}",
            events_path.display()
        )
    })?;

    let mut findings = Vec::new();
    let trusted_resume_terminal_skips = trusted_resume_terminal_skips(&event_log);
    let legacy_attempt_metadata = legacy_attempt_metadata(&event_log);
    verify_plan_ids(receipt, &rebuilt_state, &mut findings);
    verify_state_matches_events(
        state,
        &rebuilt_state,
        &trusted_resume_terminal_skips,
        legacy_attempt_metadata,
        &mut findings,
    );
    verify_receipt_matches_state(state, receipt, &mut findings);
    verify_receipt_coverage(&event_log, state, receipt, &mut findings);
    verify_reconciliation_evidence(
        &event_log,
        &receipt.plan_id,
        reconciliation_report,
        &mut findings,
    );

    if findings.is_empty() {
        return Ok(());
    }

    bail!(
        "release evidence drift detected; refusing to write receipt.json\n{}",
        findings
            .into_iter()
            .map(|finding| format!("  - {finding}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn verify_plan_ids(receipt: &Receipt, rebuilt_state: &ExecutionState, findings: &mut Vec<String>) {
    if rebuilt_state.plan_id != receipt.plan_id {
        findings.push(format!(
            "events plan_id {} does not match receipt plan_id {}",
            rebuilt_state.plan_id, receipt.plan_id
        ));
    }
}

fn verify_state_matches_events(
    state: &ExecutionState,
    rebuilt_state: &ExecutionState,
    trusted_resume_terminal_skips: &BTreeSet<String>,
    legacy_attempt_metadata: bool,
    findings: &mut Vec<String>,
) {
    for (key, progress) in &state.packages {
        match rebuilt_state.packages.get(key) {
            Some(event_progress) if event_progress.state == progress.state => {}
            Some(event_progress)
                if matches!(
                    (&progress.state, &event_progress.state),
                    (PackageState::Published, PackageState::Skipped { .. })
                ) && trusted_resume_terminal_skips.contains(key) => {}
            Some(event_progress)
                if matches!(
                    (&progress.state, &event_progress.state),
                    (PackageState::Skipped { .. }, PackageState::Skipped { .. })
                ) && trusted_resume_terminal_skips.contains(key) => {}
            Some(event_progress) => findings.push(format!(
                "{key} state drift: state.json says {}; events project {}",
                state_name(&progress.state),
                state_name(&event_progress.state)
            )),
            None if matches!(progress.state, PackageState::Pending) => {}
            None => findings.push(format!(
                "{key} state drift: state.json says {} but events contain no package projection",
                state_name(&progress.state)
            )),
        }
    }

    for (key, progress) in &rebuilt_state.packages {
        if !state.packages.contains_key(key) {
            findings.push(format!(
                "{key} state drift: events project {} but state.json has no package entry",
                state_name(&progress.state)
            ));
        }
    }

    if !attempt_histories_match(
        state,
        rebuilt_state,
        trusted_resume_terminal_skips,
        legacy_attempt_metadata,
    ) {
        let mut detail = format!(
            "attempt_history drift: state has {} entries but events project {} entries",
            state.attempt_history.len(),
            rebuilt_state.attempt_history.len()
        );
        let pairs = state
            .attempt_history
            .iter()
            .zip(rebuilt_state.attempt_history.iter())
            .enumerate();
        for (idx, (live, rebuilt)) in pairs {
            if live == rebuilt {
                continue;
            }
            detail.push_str(&format!(
                "; entry[{idx}] differs (live {}@{} attempt={}/{} class={:?} next={:?} started={} ended={} msg={:?}; events {}@{} attempt={}/{} class={:?} next={:?} started={} ended={} msg={:?})",
                live.package,
                live.version,
                live.attempt,
                live.max_attempts,
                live.error_class,
                live.next_attempt_at.is_some(),
                live.started_at.to_rfc3339(),
                live.ended_at.to_rfc3339(),
                live.redacted_message,
                rebuilt.package,
                rebuilt.version,
                rebuilt.attempt,
                rebuilt.max_attempts,
                rebuilt.error_class,
                rebuilt.next_attempt_at.is_some(),
                rebuilt.started_at.to_rfc3339(),
                rebuilt.ended_at.to_rfc3339(),
                rebuilt.redacted_message,
            ));
        }
        if state.attempt_history.len() != rebuilt_state.attempt_history.len() {
            detail.push_str(&format!(
                "; live_keys={:?}; event_keys={:?}",
                state
                    .attempt_history
                    .iter()
                    .map(|a| format!("{}@{}#{}", a.package, a.version, a.attempt))
                    .collect::<Vec<_>>(),
                rebuilt_state
                    .attempt_history
                    .iter()
                    .map(|a| format!("{}@{}#{}", a.package, a.version, a.attempt))
                    .collect::<Vec<_>>(),
            ));
        }
        findings.push(detail);
    }
}

fn legacy_attempt_metadata(event_log: &EventLog) -> bool {
    // 0.4 serialized PackageAttempted without max_attempts. Serde therefore
    // leaves it at zero, which the rebuild path treats as the attempt number.
    // Modern writers always persist a nonzero max_attempts value.
    let attempts = event_log
        .all_events()
        .iter()
        .filter_map(|event| match &event.event_type {
            EventType::PackageAttempted { max_attempts, .. } => Some(*max_attempts),
            _ => None,
        });

    let mut saw_attempt = false;
    for max_attempts in attempts {
        saw_attempt = true;
        if max_attempts != 0 {
            return false;
        }
    }
    saw_attempt
}

fn attempt_histories_match(
    state: &ExecutionState,
    rebuilt_state: &ExecutionState,
    trusted_resume_terminal_skips: &BTreeSet<String>,
    legacy_attempt_metadata: bool,
) -> bool {
    if state.attempt_history == rebuilt_state.attempt_history {
        return true;
    }

    if !legacy_attempt_metadata
        || state.attempt_history.len() != rebuilt_state.attempt_history.len()
    {
        return false;
    }

    // 0.4 state ended an attempt when upload completed, while its later
    // readiness and PackagePublished events let the event projection end it
    // later. Compare the durable identity and failure semantics strictly, and
    // leave only those legacy-unknown timing/retry fields tolerant.
    state
        .attempt_history
        .iter()
        .zip(rebuilt_state.attempt_history.iter())
        .all(|(live, rebuilt)| {
            let key = format!("{}@{}", live.package, live.version);
            let same_terminal_state = state
                .packages
                .get(&key)
                .and_then(|progress| {
                    rebuilt_state
                        .packages
                        .get(&key)
                        .map(|event| (progress, event))
                })
                .is_some_and(|(progress, event)| {
                    progress.state == event.state
                        || (matches!(
                            (&progress.state, &event.state),
                            (PackageState::Published, PackageState::Skipped { .. })
                        ) && trusted_resume_terminal_skips.contains(&key))
                });

            same_terminal_state
                && live.package == rebuilt.package
                && live.version == rebuilt.version
                && live.attempt == rebuilt.attempt
                && live.error_class == rebuilt.error_class
                && live.next_attempt_at == rebuilt.next_attempt_at
                && live.redacted_message == rebuilt.redacted_message
                && live.started_at == rebuilt.started_at
                && live.ended_at <= rebuilt.ended_at
                && rebuilt.max_attempts == rebuilt.attempt
                && live.max_attempts >= rebuilt.max_attempts
        })
}

fn trusted_resume_terminal_skips(event_log: &EventLog) -> BTreeSet<String> {
    event_log
        .all_events()
        .iter()
        .filter_map(|event| match &event.event_type {
            EventType::PackageSkipped { reason }
                if reason.starts_with("resume: state already ") =>
            {
                Some(event.package.clone())
            }
            _ => None,
        })
        .collect()
}

fn verify_receipt_matches_state(
    state: &ExecutionState,
    receipt: &Receipt,
    findings: &mut Vec<String>,
) {
    for package in &receipt.packages {
        let key = format!("{}@{}", package.name, package.version);
        match state.packages.get(&key) {
            Some(progress) if progress.state == package.state => {}
            Some(progress) => findings.push(format!(
                "{key} receipt drift: receipt says {}; state.json says {}",
                state_name(&package.state),
                state_name(&progress.state)
            )),
            None => findings.push(format!(
                "{key} receipt drift: receipt has package but state.json has no package entry"
            )),
        }
    }
}

/// Check the coverage that historical events can establish. A current trusted
/// terminal-skip event is compatible with an omitted predecessor, but the same
/// event is also emitted for selected terminal packages. It cannot establish
/// the original resume scope. Runtime finalization checks exact selected
/// membership separately with the invocation inputs.
fn verify_receipt_coverage(
    event_log: &EventLog,
    state: &ExecutionState,
    receipt: &Receipt,
    findings: &mut Vec<String>,
) {
    let all_events = event_log.all_events();
    let start = all_events
        .iter()
        .rposition(|event| matches!(event.event_type, EventType::ExecutionStarted))
        .unwrap_or(0);
    let Some(current_events) = all_events.get(start..) else {
        findings.push("receipt coverage drift: current event segment is unavailable".to_string());
        return;
    };
    let receipted: BTreeSet<String> = receipt
        .packages
        .iter()
        .map(|package| format!("{}@{}", package.name, package.version))
        .collect();
    if receipted.len() != receipt.packages.len() {
        findings.push("receipt contains duplicate package entries".to_string());
    }
    for (key, progress) in &state.packages {
        let terminal = matches!(
            progress.state,
            PackageState::Published | PackageState::Skipped { .. }
        );
        if receipt.execution_result == ExecutionResult::Success && !terminal {
            findings.push(format!(
                "{key}: success receipt conflicts with {} durable state",
                state_name(&progress.state)
            ));
        }
        if receipted.contains(key) {
            continue;
        }
        let trusted_terminal_omission = terminal && current_events.iter().any(|event| {
            event.package == *key && matches!(&event.event_type,
                EventType::PackageSkipped { reason } if reason.starts_with("resume: state already "))
        });
        let untouched_nonterminal_prefix = !terminal
            && receipt.execution_result != ExecutionResult::Success
            && !current_events.iter().any(|event| event.package == *key);
        if !trusted_terminal_omission && !untouched_nonterminal_prefix {
            findings.push(format!("{key} receipt coverage drift: package is omitted without compatible current resume-skip evidence"));
        }
    }
}

fn verify_reconciliation_evidence(
    event_log: &EventLog,
    expected_plan_id: &str,
    reconciliation_report: Option<&ReconciliationReport>,
    findings: &mut Vec<String>,
) {
    let mut reconciled_outcomes: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for event in event_log.all_events() {
        if let EventType::PublishReconciled { outcome } = &event.event_type {
            reconciled_outcomes
                .entry(event.package.clone())
                .or_default()
                .push(outcome.clone());
        }
    }
    let reconciled_packages: BTreeSet<String> = reconciled_outcomes.keys().cloned().collect();

    if reconciled_packages.is_empty() {
        if reconciliation_report.is_some() {
            findings.push(
                "reconciliation.json is present but events contain no reconciliation outcomes"
                    .to_string(),
            );
        }
        return;
    }

    let Some(report) = reconciliation_report else {
        findings.push(format!(
            "events contain reconciliation outcomes for {} but reconciliation.json was not produced",
            reconciled_packages.into_iter().collect::<Vec<_>>().join(", ")
        ));
        return;
    };

    if report.plan_id != expected_plan_id {
        findings.push(format!(
            "reconciliation plan_id {} does not match expected plan_id {}",
            report.plan_id, expected_plan_id
        ));
    }

    let report_packages: BTreeSet<String> = report
        .records
        .iter()
        .map(|record| record.package.clone())
        .collect();
    let mut report_outcomes: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for record in &report.records {
        report_outcomes
            .entry(record.package.clone())
            .or_default()
            .push(record.outcome.clone());
    }

    for package in reconciled_packages.difference(&report_packages) {
        findings.push(format!(
            "{package} reconciliation drift: event outcome is missing from reconciliation.json"
        ));
    }
    for package in report_packages.difference(&reconciled_packages) {
        findings.push(format!(
            "{package} reconciliation drift: reconciliation.json has no matching event outcome"
        ));
    }
    for (package, event_history) in reconciled_outcomes {
        if let Some(report_history) = report_outcomes.get(package.as_str())
            && report_history.as_slice() != event_history.as_slice()
        {
            findings.push(format!(
                "{package} reconciliation drift: ordered event and reconciliation.json outcome histories differ"
            ));
        }
    }
}

fn state_name(state: &PackageState) -> &'static str {
    match state {
        PackageState::Pending => "pending",
        PackageState::Uploaded => "uploaded",
        PackageState::Published => "published",
        PackageState::Skipped { .. } => "skipped",
        PackageState::Failed { .. } => "failed",
        PackageState::Ambiguous { .. } => "ambiguous",
        PackageState::CleanupIncomplete { .. } => "cleanup_incomplete",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{Duration, Utc};
    use shipper_types::{
        AttemptDetail, EnvironmentFingerprint, ErrorClass, PackageEvidence, PackageProgress,
        PackageReceipt, PublishEvent, ReconciliationEvidenceKind, ReconciliationEvidenceSource,
        ReconciliationOperatorAction, ReconciliationOutcome, ReconciliationRecord,
        ReconciliationReport, ReconciliationTrigger, Registry,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::state::events::{EVENTS_FILE, EventLog, events_path};

    fn pkg_progress(name: &str, version: &str, state: PackageState) -> (String, PackageProgress) {
        let key = format!("{name}@{version}");
        (
            key,
            PackageProgress {
                name: name.to_string(),
                version: version.to_string(),
                attempts: 1,
                state,
                last_updated_at: Utc::now(),
            },
        )
    }

    fn make_state(packages: Vec<(String, PackageProgress)>) -> ExecutionState {
        ExecutionState {
            state_version: "test".to_string(),
            plan_id: "test-plan".to_string(),
            registry: Registry {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
            },
            attempt_history: Vec::new(),
            packages: packages.into_iter().collect::<BTreeMap<_, _>>(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn write_events(dir: &Path, events: Vec<PublishEvent>) {
        let mut log = EventLog::new();
        for e in events {
            log.record(e);
        }
        log.write_to_file(&dir.join(EVENTS_FILE))
            .expect("write events");
    }

    fn published_event(name: &str, version: &str) -> PublishEvent {
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackagePublished { duration_ms: 10 },
            package: format!("{name}@{version}"),
        }
    }

    fn plan_created_event() -> PublishEvent {
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PlanCreated {
                plan_id: "test-plan".to_string(),
                package_count: 1,
            },
            package: "all".to_string(),
        }
    }

    fn receipt(packages: Vec<PackageReceipt>) -> Receipt {
        Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "test-plan".to_string(),
            registry: Registry::crates_io(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            packages,
            event_log_path: Path::new(".shipper/events.jsonl").to_path_buf(),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "test".to_string(),
                cargo_version: None,
                rust_version: None,
                os: "test".to_string(),
                arch: "test".to_string(),
            },
            auth_evidence: None,
            execution_result: crate::types::ExecutionResult::Success,
        }
    }

    fn package_receipt(name: &str, version: &str, state: PackageState) -> PackageReceipt {
        PackageReceipt {
            name: name.to_string(),
            version: version.to_string(),
            attempts: 1,
            state,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 10,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        }
    }

    fn attempt_detail(
        package: &str,
        version: &str,
        attempt: u32,
        started_at: chrono::DateTime<Utc>,
    ) -> AttemptDetail {
        AttemptDetail {
            package: package.to_string(),
            version: version.to_string(),
            attempt,
            max_attempts: attempt,
            started_at,
            ended_at: started_at,
            error_class: Some(ErrorClass::Retryable),
            next_attempt_at: None,
            redacted_message: Some("retryable failure".to_string()),
        }
    }

    fn reconciliation_report(package: &str) -> ReconciliationReport {
        let (name, version) = package.rsplit_once('@').expect("package label");
        ReconciliationReport {
            schema_version: "shipper.reconciliation.v1".to_string(),
            plan_id: "test-plan".to_string(),
            registry: Registry::crates_io(),
            generated_at: Utc::now(),
            evidence_sources: vec![ReconciliationEvidenceSource {
                kind: ReconciliationEvidenceKind::EventLog,
                path: ".shipper/events.jsonl".to_string(),
            }],
            records: vec![ReconciliationRecord {
                package: package.to_string(),
                name: name.to_string(),
                version: version.to_string(),
                trigger: ReconciliationTrigger::CargoAmbiguousExit,
                method: None,
                cargo_exit_class: Some(shipper_types::ErrorClass::Ambiguous),
                outcome: shipper_types::ReconciliationOutcome::Published {
                    attempts: 1,
                    elapsed_ms: 10,
                },
                operator_action: ReconciliationOperatorAction::MarkPublishedContinue,
            }],
        }
    }

    #[test]
    fn unfinished_consistency_requires_matching_state_and_reconciliation_outcomes() -> Result<()> {
        let td = tempdir()?;
        let unknown = ReconciliationOutcome::StillUnknown {
            attempts: 1,
            elapsed_ms: 10,
            reason: "unknown".to_string(),
        };
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStarted,
                    package: "all".to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageUploaded,
                    package: "a@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: unknown.clone(),
                    },
                    package: "a@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::Published {
                            attempts: 2,
                            elapsed_ms: 20,
                        },
                    },
                    package: "a@1.0.0".to_string(),
                },
            ],
        );
        let state = rebuild_state_from_events(
            &events_path(td.path()),
            StateRebuildOptions::new(Registry::crates_io()),
        )?;
        let mut report = reconciliation_report("a@1.0.0");
        let published_record = report
            .records
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing reconciliation record"))?;
        let mut unknown_record = published_record.clone();
        unknown_record.outcome = unknown;
        let mut final_record = published_record;
        final_record.outcome = ReconciliationOutcome::Published {
            attempts: 2,
            elapsed_ms: 20,
        };
        report.records = vec![unknown_record, final_record];
        verify_unfinished_consistency(&events_path(td.path()), &state, Some(&report))?;

        let record = report
            .records
            .get_mut(0)
            .ok_or_else(|| anyhow::anyhow!("missing reconciliation record"))?;
        record.outcome = ReconciliationOutcome::Published {
            attempts: 2,
            elapsed_ms: 20,
        };
        anyhow::ensure!(
            verify_unfinished_consistency(&events_path(td.path()), &state, Some(&report)).is_err(),
            "an earlier reconciliation mismatch must not be hidden by a matching final outcome"
        );
        Ok(())
    }

    #[test]
    fn controlled_stop_rejects_stale_not_published_before_latest_still_unknown() -> Result<()> {
        let td = tempdir()?;
        let package = "a@1.0.0";
        let not_published = ReconciliationOutcome::NotPublished {
            attempts: 1,
            elapsed_ms: 10,
        };
        let still_unknown = ReconciliationOutcome::StillUnknown {
            attempts: 1,
            elapsed_ms: 20,
            reason: "registry unavailable".to_string(),
        };
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStarted,
                    package: "all".to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: not_published.clone(),
                    },
                    package: package.to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: still_unknown.clone(),
                    },
                    package: package.to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "forged retryable tail".to_string(),
                    },
                    package: package.to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStopped {
                        reason:
                            shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
                    },
                    package: package.to_string(),
                },
            ],
        );
        let state = rebuild_state_from_events(
            &events_path(td.path()),
            StateRebuildOptions::new(Registry::crates_io()),
        )?;
        let mut report = reconciliation_report(package);
        let template = report
            .records
            .first()
            .cloned()
            .context("reconciliation fixture record")?;
        report.records = vec![
            ReconciliationRecord {
                outcome: not_published,
                operator_action: ReconciliationOperatorAction::RetryAllowed,
                ..template.clone()
            },
            ReconciliationRecord {
                outcome: still_unknown,
                operator_action: ReconciliationOperatorAction::OperatorActionRequired,
                ..template
            },
        ];
        let error =
            verify_controlled_stop_consistency(&events_path(td.path()), &state, Some(&report))
                .expect_err("latest StillUnknown must block controlled resume");
        assert!(format!("{error:#}").contains("latest StillUnknown"));
        Ok(())
    }

    #[test]
    fn controlled_stop_rejects_latest_still_unknown_for_another_package() -> Result<()> {
        let td = tempdir()?;
        let marker_package = "a@1.0.0";
        let other_package = "b@1.0.0";
        let not_published = ReconciliationOutcome::NotPublished {
            attempts: 1,
            elapsed_ms: 10,
        };
        let still_unknown = ReconciliationOutcome::StillUnknown {
            attempts: 1,
            elapsed_ms: 20,
            reason: "other package remains ambiguous".to_string(),
        };
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStarted,
                    package: "all".to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: still_unknown.clone(),
                    },
                    package: other_package.to_string(),
                },
                published_event("b", "1.0.0"),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: not_published.clone(),
                    },
                    package: marker_package.to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "retry budget exhausted".to_string(),
                    },
                    package: marker_package.to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStopped {
                        reason:
                            shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
                    },
                    package: marker_package.to_string(),
                },
            ],
        );
        let state = rebuild_state_from_events(
            &events_path(td.path()),
            StateRebuildOptions::new(Registry::crates_io()),
        )?;
        let mut report = reconciliation_report(marker_package);
        let template = report
            .records
            .first()
            .cloned()
            .context("reconciliation template")?;
        report.records = vec![
            ReconciliationRecord {
                package: other_package.to_string(),
                name: "b".to_string(),
                version: "1.0.0".to_string(),
                outcome: still_unknown,
                operator_action: ReconciliationOperatorAction::OperatorActionRequired,
                ..template.clone()
            },
            ReconciliationRecord {
                outcome: not_published,
                operator_action: ReconciliationOperatorAction::RetryAllowed,
                ..template
            },
        ];
        let error =
            verify_controlled_stop_consistency(&events_path(td.path()), &state, Some(&report))
                .expect_err("another package's latest StillUnknown must block controlled resume");
        assert!(format!("{error:#}").contains("latest StillUnknown"));
        assert!(has_not_published_retryable_posture(&state, Some(&report)));
        assert!(!has_not_published_retryable_posture(&state, None));
        Ok(())
    }

    #[test]
    fn controlled_stop_rejects_mismatched_duplicate_reconciliation_identity() -> Result<()> {
        let td = tempdir()?;
        let package = "a@1.0.0";
        let not_published = ReconciliationOutcome::NotPublished {
            attempts: 1,
            elapsed_ms: 10,
        };
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStarted,
                    package: "all".to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: not_published.clone(),
                    },
                    package: package.to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "retry budget exhausted".to_string(),
                    },
                    package: package.to_string(),
                },
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStopped {
                        reason:
                            shipper_types::ControlledStopReason::NotPublishedRetryBudgetExhausted,
                    },
                    package: package.to_string(),
                },
            ],
        );
        let state = rebuild_state_from_events(
            &events_path(td.path()),
            StateRebuildOptions::new(Registry::crates_io()),
        )?;
        let mut report = reconciliation_report(package);
        let mut valid = report
            .records
            .first()
            .cloned()
            .context("reconciliation fixture record")?;
        valid.outcome = not_published;
        valid.operator_action = ReconciliationOperatorAction::RetryAllowed;
        let corrupt_duplicate = ReconciliationRecord {
            name: "forged".to_string(),
            ..valid.clone()
        };
        report.records = vec![valid, corrupt_duplicate];

        assert!(!has_not_published_retryable_posture(&state, Some(&report)));
        let error =
            verify_controlled_stop_consistency(&events_path(td.path()), &state, Some(&report))
                .expect_err("duplicated package identity must not authorize recovery");
        assert!(
            format!("{error:#}").contains("reconciliation package identity disagrees"),
            "{error:#}"
        );
        Ok(())
    }

    #[test]
    fn unfinished_consistency_rejects_uploaded_state_and_report_without_events() -> Result<()> {
        let td = tempdir()?;
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStarted,
                    package: "all".to_string(),
                },
            ],
        );
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Uploaded)]);
        let mut report = reconciliation_report("a@1.0.0");
        let record = report
            .records
            .get_mut(0)
            .ok_or_else(|| anyhow::anyhow!("missing reconciliation record"))?;
        record.outcome = ReconciliationOutcome::StillUnknown {
            attempts: 1,
            elapsed_ms: 10,
            reason: "unknown".to_string(),
        };
        anyhow::ensure!(
            verify_unfinished_consistency(&events_path(td.path()), &state, Some(&report)).is_err(),
            "state and reconciliation evidence absent from events should fail consistency"
        );
        Ok(())
    }

    #[test]
    fn consistent_when_state_and_events_agree() {
        let td = tempdir().expect("tempdir");
        write_events(
            td.path(),
            vec![published_event("a", "1.0.0"), published_event("b", "2.0.0")],
        );

        let state = make_state(vec![
            pkg_progress("a", "1.0.0", PackageState::Published),
            pkg_progress("b", "2.0.0", PackageState::Published),
        ]);

        let drift = verify_events_state_consistency(&td.path().join(EVENTS_FILE), &state)
            .expect("check runs");
        assert!(drift.is_consistent(), "expected no drift; got {:?}", drift);
    }

    #[test]
    fn consistent_when_resume_skip_documents_published_state() {
        let td = tempdir().expect("tempdir");
        write_events(
            td.path(),
            vec![PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PackageSkipped {
                    reason: "resume: state already published".to_string(),
                },
                package: "a@1.0.0".to_string(),
            }],
        );

        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);

        let drift = verify_events_state_consistency(&td.path().join(EVENTS_FILE), &state)
            .expect("check runs");
        assert!(drift.is_consistent(), "expected no drift; got {:?}", drift);
    }

    #[test]
    fn detects_in_events_only() {
        // events says published, state says pending â†’ resume would duplicate
        let td = tempdir().expect("tempdir");
        write_events(td.path(), vec![published_event("a", "1.0.0")]);

        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Pending)]);

        let drift = verify_events_state_consistency(&td.path().join(EVENTS_FILE), &state)
            .expect("check runs");
        assert!(!drift.is_consistent());
        assert_eq!(drift.in_events_only, vec!["a@1.0.0".to_string()]);
        assert!(drift.in_state_only.is_empty());
    }

    #[test]
    fn detects_in_state_only() {
        // state says published but no event recorded it â†’ event log bypassed
        let td = tempdir().expect("tempdir");
        write_events(td.path(), vec![]);

        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);

        let drift = verify_events_state_consistency(&td.path().join(EVENTS_FILE), &state)
            .expect("check runs");
        assert!(!drift.is_consistent());
        assert_eq!(drift.in_state_only, vec!["a@1.0.0".to_string()]);
        assert!(drift.in_events_only.is_empty());
    }

    #[test]
    fn drift_finalization_rejects_state_published_without_event_projection() {
        let td = tempdir().expect("tempdir");
        write_events(td.path(), vec![plan_created_event()]);
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);
        let receipt = receipt(vec![package_receipt("a", "1.0.0", PackageState::Published)]);

        let err =
            verify_finalization_consistency(&td.path().join(EVENTS_FILE), &state, &receipt, None)
                .expect_err("state/event drift should fail finalization");

        assert!(
            err.to_string().contains("release evidence drift detected"),
            "{err:#}"
        );
        assert!(
            err.to_string()
                .contains("state.json says published but events contain no package projection"),
            "{err:#}"
        );
    }

    #[test]
    fn whole_plan_completion_oracle_rejects_missing_or_nonterminal_prefixes() {
        let plan = ReleasePlan {
            plan_version: "1".into(),
            plan_id: "test-plan".into(),
            created_at: Utc::now(),
            registry: Registry::crates_io(),
            packages: ["a", "b"]
                .into_iter()
                .map(|name| shipper_types::PlannedPackage {
                    name: name.into(),
                    version: "1.0.0".into(),
                    manifest_path: format!("{name}/Cargo.toml").into(),
                    regime: None,
                })
                .collect(),
            dependencies: BTreeMap::new(),
        };
        let mut state = make_state(vec![
            pkg_progress("a", "1.0.0", PackageState::Published),
            pkg_progress("b", "1.0.0", PackageState::Published),
        ]);
        let receipt = receipt(vec![package_receipt("b", "1.0.0", PackageState::Published)]);
        verify_plan_completion(&plan, &state, &receipt).expect("terminal prefix may be omitted");
        state.packages.get_mut("a@1.0.0").expect("prefix").name = "wrong-package".into();
        let error =
            verify_plan_completion(&plan, &state, &receipt).expect_err("malformed prefix identity");
        assert!(error.to_string().contains("identity does not match"));
        state.packages.get_mut("a@1.0.0").expect("prefix").name = "a".into();
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
            state.packages.get_mut("a@1.0.0").expect("prefix").state = prefix;
            assert!(verify_plan_completion(&plan, &state, &receipt).is_err());
        }
        state.packages.remove("a@1.0.0");
        assert!(
            verify_plan_completion(&plan, &state, &receipt).is_err(),
            "missing is not completion evidence"
        );
    }

    #[test]
    fn whole_plan_completion_distinguishes_empty_plan_from_empty_receipt() {
        let mut state = make_state(vec![]);
        let mut plan = ReleasePlan {
            plan_version: "1".into(),
            plan_id: state.plan_id.clone(),
            created_at: Utc::now(),
            registry: state.registry.clone(),
            packages: vec![],
            dependencies: BTreeMap::new(),
        };
        let receipt = receipt(vec![]);
        verify_plan_completion(&plan, &state, &receipt)
            .expect("a genuinely empty plan permits an empty receipt");

        plan.packages.push(shipper_types::PlannedPackage {
            name: "a".into(),
            version: "1.0.0".into(),
            manifest_path: "a/Cargo.toml".into(),
            regime: None,
        });
        let (key, progress) = pkg_progress("a", "1.0.0", PackageState::Published);
        state.packages.insert(key, progress);
        let error = verify_plan_completion(&plan, &state, &receipt)
            .expect_err("a nonempty plan must retain at least one selected receipt entry");
        assert!(error.to_string().contains("empty execution receipt"));
    }

    #[test]
    fn receipt_coverage_rejects_unexplained_omission_and_stale_resume_skip() {
        let td = tempdir().expect("tempdir");
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);
        for previous in [
            published_event("a", "1.0.0"),
            PublishEvent {
                timestamp: Utc::now(),
                package: "a@1.0.0".into(),
                event_type: EventType::PackageSkipped {
                    reason: "resume: state already published".into(),
                },
            },
        ] {
            write_events(
                td.path(),
                vec![
                    plan_created_event(),
                    previous,
                    PublishEvent {
                        timestamp: Utc::now(),
                        package: "all".into(),
                        event_type: EventType::ExecutionStarted,
                    },
                    published_event("a", "1.0.0"),
                ],
            );
            let receipt = receipt(vec![]);
            let error = verify_finalization_consistency(
                &td.path().join(EVENTS_FILE),
                &state,
                &receipt,
                None,
            )
            .expect_err("omission has no compatible current skip evidence");
            assert!(
                error.to_string().contains("receipt coverage drift"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn receipt_coverage_preserves_empty_and_latest_segment_boundaries() {
        let started = || PublishEvent {
            timestamp: Utc::now(),
            package: "all".into(),
            event_type: EventType::ExecutionStarted,
        };
        let skipped = || PublishEvent {
            timestamp: Utc::now(),
            package: "a@1.0.0".into(),
            event_type: EventType::PackageSkipped {
                reason: "resume: state already published".into(),
            },
        };
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);
        for (label, events, accepted) in [
            ("empty history", vec![], false),
            ("no marker with skip", vec![skipped()], true),
            (
                "no marker without skip",
                vec![published_event("a", "1.0.0")],
                false,
            ),
            ("latest marker only", vec![skipped(), started()], false),
            ("current skip", vec![started(), skipped()], true),
            (
                "old segment skip",
                vec![started(), skipped(), started()],
                false,
            ),
            (
                "latest of multiple segments",
                vec![started(), skipped(), started(), skipped()],
                true,
            ),
        ] {
            let mut log = EventLog::new();
            for event in events {
                log.record(event);
            }
            let mut findings = Vec::new();
            verify_receipt_coverage(&log, &state, &receipt(vec![]), &mut findings);
            assert_eq!(findings.is_empty(), accepted, "{label}: {findings:?}");
            if !accepted {
                assert!(
                    findings
                        .iter()
                        .any(|finding| finding.contains("receipt coverage drift")),
                    "{label}: {findings:?}"
                );
            }
        }

        let mut findings = Vec::new();
        verify_receipt_coverage(
            &EventLog::new(),
            &make_state(vec![]),
            &receipt(vec![]),
            &mut findings,
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn partial_receipt_preserves_untouched_pending_prefix_without_fabrication() {
        let td = tempdir().expect("tempdir");
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    package: "all".into(),
                    event_type: EventType::ExecutionStarted,
                },
                published_event("b", "1.0.0"),
            ],
        );
        let state = make_state(vec![
            pkg_progress("a", "1.0.0", PackageState::Pending),
            pkg_progress("b", "1.0.0", PackageState::Published),
        ]);
        let mut receipt = receipt(vec![package_receipt("b", "1.0.0", PackageState::Published)]);
        receipt.execution_result = ExecutionResult::PartialFailure;
        verify_finalization_consistency(&td.path().join(EVENTS_FILE), &state, &receipt, None)
            .expect("partial invocation retains pending prefix in state, not fabricated receipts");
        receipt.execution_result = ExecutionResult::Success;
        let error =
            verify_finalization_consistency(&td.path().join(EVENTS_FILE), &state, &receipt, None)
                .expect_err("changing only completion to success must fail");
        assert!(
            error
                .to_string()
                .contains("success receipt conflicts with pending"),
            "{error:#}"
        );
    }

    #[test]
    fn drift_finalization_accepts_reconciled_published_projection() {
        let td = tempdir().expect("tempdir");
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: shipper_types::ReconciliationOutcome::Published {
                            attempts: 1,
                            elapsed_ms: 10,
                        },
                    },
                    package: "a@1.0.0".to_string(),
                },
            ],
        );
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);
        let receipt = receipt(vec![package_receipt("a", "1.0.0", PackageState::Published)]);
        let report = reconciliation_report("a@1.0.0");

        verify_finalization_consistency(
            &td.path().join(EVENTS_FILE),
            &state,
            &receipt,
            Some(&report),
        )
        .expect("reconciled published event should project to published state");
    }

    #[test]
    fn legacy_attempt_metadata_does_not_block_resume_finalization() {
        let td = tempdir().expect("tempdir");
        let attempt_started = Utc::now();
        let upload_finished = attempt_started + Duration::seconds(2);
        let published = upload_finished + Duration::seconds(1);
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: attempt_started,
                    event_type: EventType::PackageStarted {
                        name: "legacy".to_string(),
                        version: "0.4.0".to_string(),
                    },
                    package: "legacy@0.4.0".to_string(),
                },
                PublishEvent {
                    timestamp: attempt_started,
                    event_type: EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 0,
                    },
                    package: "legacy@0.4.0".to_string(),
                },
                PublishEvent {
                    timestamp: upload_finished,
                    event_type: EventType::PackageOutput {
                        stdout_tail: String::new(),
                        stderr_tail: String::new(),
                    },
                    package: "legacy@0.4.0".to_string(),
                },
                PublishEvent {
                    timestamp: published,
                    event_type: EventType::PackagePublished { duration_ms: 3_000 },
                    package: "legacy@0.4.0".to_string(),
                },
            ],
        );

        let mut state = make_state(vec![pkg_progress(
            "legacy",
            "0.4.0",
            PackageState::Published,
        )]);
        state.state_version = "shipper.state.v1".to_string();
        state.attempt_history = vec![AttemptDetail {
            package: "legacy".to_string(),
            version: "0.4.0".to_string(),
            attempt: 1,
            max_attempts: 12,
            started_at: attempt_started,
            ended_at: upload_finished,
            error_class: None,
            next_attempt_at: None,
            redacted_message: None,
        }];

        let receipt = receipt(vec![package_receipt(
            "legacy",
            "0.4.0",
            PackageState::Published,
        )]);

        verify_finalization_consistency(&td.path().join(EVENTS_FILE), &state, &receipt, None)
            .expect("legacy unknown attempt metadata should remain resumable");
    }

    #[test]
    fn drift_finalization_accepts_trusted_resume_published_skip_projection() {
        let td = tempdir().expect("tempdir");
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageSkipped {
                        reason: "resume: state already published".to_string(),
                    },
                    package: "a@1.0.0".to_string(),
                },
            ],
        );
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);
        let receipt = receipt(vec![]);

        verify_finalization_consistency(&td.path().join(EVENTS_FILE), &state, &receipt, None)
            .expect("trusted resume skip should not force receipt drift");
    }

    #[test]
    fn drift_finalization_requires_reconciliation_report_for_reconciled_events() {
        let td = tempdir().expect("tempdir");
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PublishReconciled {
                        outcome: shipper_types::ReconciliationOutcome::Published {
                            attempts: 1,
                            elapsed_ms: 10,
                        },
                    },
                    package: "a@1.0.0".to_string(),
                },
            ],
        );
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);
        let receipt = receipt(vec![package_receipt("a", "1.0.0", PackageState::Published)]);

        let err =
            verify_finalization_consistency(&td.path().join(EVENTS_FILE), &state, &receipt, None)
                .expect_err("missing reconciliation report should fail finalization");

        assert!(
            err.to_string()
                .contains("reconciliation.json was not produced"),
            "{err:#}"
        );
    }

    #[test]
    fn drift_finalization_rejects_attempt_history_mismatch() {
        let td = tempdir().expect("tempdir");
        let now = Utc::now();
        write_events(
            td.path(),
            vec![
                plan_created_event(),
                PublishEvent {
                    timestamp: now,
                    event_type: EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 2,
                    },
                    package: "demo@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: now,
                    event_type: EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "retryable failure".to_string(),
                    },
                    package: "demo@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: now,
                    event_type: EventType::RetryScheduled {
                        attempt: 1,
                        max_attempts: 2,
                        delay_ms: 500,
                        next_attempt_at: now,
                        reason: ErrorClass::Retryable,
                        message: "retry again".to_string(),
                    },
                    package: "demo@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: now,
                    event_type: EventType::PackageAttempted {
                        attempt: 2,
                        command: "cargo publish".to_string(),
                        max_attempts: 2,
                    },
                    package: "demo@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: now,
                    event_type: EventType::PackageUploaded,
                    package: "demo@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: now,
                    event_type: EventType::PackagePublished { duration_ms: 10 },
                    package: "demo@1.0.0".to_string(),
                },
            ],
        );

        let events_path = td.path().join(EVENTS_FILE);
        let rebuilt = rebuild_state_from_events(
            &events_path,
            StateRebuildOptions::new(Registry::crates_io()),
        )
        .expect("rebuild");
        let mut state = rebuilt.clone();
        state
            .attempt_history
            .push(attempt_detail("demo", "1.0.0", 2, now));

        let receipt = receipt(vec![package_receipt(
            "demo",
            "1.0.0",
            PackageState::Published,
        )]);
        let err = verify_finalization_consistency(&events_path, &state, &receipt, None)
            .expect_err("attempt history mismatch should fail");

        assert!(err.to_string().contains("attempt_history drift"), "{err:#}");
    }

    #[test]
    fn drift_finalization_rejects_receipt_state_mismatch() {
        let td = tempdir().expect("tempdir");
        write_events(
            td.path(),
            vec![plan_created_event(), published_event("a", "1.0.0")],
        );
        let state = make_state(vec![pkg_progress("a", "1.0.0", PackageState::Published)]);
        let receipt = receipt(vec![package_receipt(
            "a",
            "1.0.0",
            PackageState::Skipped {
                reason: "manual mismatch".to_string(),
            },
        )]);

        let err =
            verify_finalization_consistency(&td.path().join(EVENTS_FILE), &state, &receipt, None)
                .expect_err("receipt/state drift should fail finalization");

        assert!(
            err.to_string()
                .contains("receipt drift: receipt says skipped; state.json says published"),
            "{err:#}"
        );
    }

    #[test]
    fn empty_state_and_empty_events_are_consistent() {
        let td = tempdir().expect("tempdir");
        // No events file written at all â€” read_from_file treats missing as empty.
        let state = make_state(vec![]);

        let drift = verify_events_state_consistency(&td.path().join(EVENTS_FILE), &state)
            .expect("check runs");
        assert!(drift.is_consistent());
    }

    #[test]
    fn ignores_non_published_packages() {
        // Packages in Failed/Skipped/Pending state shouldn't be checked.
        let td = tempdir().expect("tempdir");
        write_events(td.path(), vec![published_event("a", "1.0.0")]);

        let state = make_state(vec![
            pkg_progress("a", "1.0.0", PackageState::Published),
            pkg_progress(
                "b",
                "2.0.0",
                PackageState::Failed {
                    class: shipper_types::ErrorClass::Permanent,
                    message: "nope".to_string(),
                },
            ),
            pkg_progress(
                "c",
                "3.0.0",
                PackageState::Skipped {
                    reason: "already published".to_string(),
                },
            ),
            pkg_progress("d", "4.0.0", PackageState::Pending),
        ]);

        let drift = verify_events_state_consistency(&td.path().join(EVENTS_FILE), &state)
            .expect("check runs");
        assert!(
            drift.is_consistent(),
            "non-published states should not count"
        );
    }

    #[test]
    fn format_summary_consistent() {
        let drift = StateEventDrift::default();
        let s = format_drift_summary(&drift);
        assert!(s.contains("consistent"));
    }

    #[test]
    fn format_summary_mentions_both_sides() {
        let drift = StateEventDrift {
            in_events_only: vec!["a@1.0.0".to_string()],
            in_state_only: vec!["b@2.0.0".to_string()],
        };
        let s = format_drift_summary(&drift);
        assert!(s.contains("drift detected"));
        assert!(s.contains("a@1.0.0"));
        assert!(s.contains("b@2.0.0"));
    }

    // --- additional format_drift_summary edge cases ---

    #[test]
    fn format_summary_in_events_only_omits_state_section() {
        // When only events_only has entries, only that line should appear.
        let drift = StateEventDrift {
            in_events_only: vec!["a@1.0.0".to_string(), "b@2.0.0".to_string()],
            in_state_only: vec![],
        };
        let s = format_drift_summary(&drift);
        assert!(s.contains("drift detected"));
        assert!(s.contains("published in events.jsonl but NOT in state.json (2)"));
        assert!(s.contains("a@1.0.0, b@2.0.0"));
        // The state-only section MUST be suppressed when empty.
        assert!(
            !s.contains("marked published in state.json"),
            "state-only section must be omitted; got: {s}"
        );
    }

    #[test]
    fn format_summary_in_state_only_omits_events_section() {
        let drift = StateEventDrift {
            in_events_only: vec![],
            in_state_only: vec!["c@3.0.0".to_string()],
        };
        let s = format_drift_summary(&drift);
        assert!(s.contains("drift detected"));
        assert!(s.contains("marked published in state.json but NO event (1)"));
        assert!(s.contains("c@3.0.0"));
        // The events-only section MUST be suppressed when empty.
        assert!(
            !s.contains("published in events.jsonl but NOT in state.json"),
            "events-only section must be omitted; got: {s}"
        );
    }

    #[test]
    fn format_summary_lists_counts_and_joined_names() {
        // Three names on each side; count and join formatting should match.
        let drift = StateEventDrift {
            in_events_only: vec!["a@1".to_string(), "b@2".to_string(), "c@3".to_string()],
            in_state_only: vec!["x@1".to_string(), "y@2".to_string()],
        };
        let s = format_drift_summary(&drift);
        // Counts.
        assert!(s.contains("events.jsonl but NOT in state.json (3)"));
        assert!(s.contains("marked published in state.json but NO event (2)"));
        // Comma-space joining.
        assert!(s.contains("a@1, b@2, c@3"));
        assert!(s.contains("x@1, y@2"));
        // Authoritative-source breadcrumb is always present in the header.
        assert!(s.contains("events.jsonl is authoritative"));
    }

    #[test]
    fn format_summary_consistent_is_single_line() {
        // The "all good" branch should stay compact: no header, no bullets.
        let s = format_drift_summary(&StateEventDrift::default());
        assert!(s.contains("consistent"));
        assert!(s.contains("events.jsonl"));
        assert!(s.contains("state.json"));
        assert!(!s.contains('\n'));
    }
}
