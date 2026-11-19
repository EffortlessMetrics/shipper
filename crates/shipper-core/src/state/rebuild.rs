//! Rebuild `state.json` from the authoritative event log.
//!
//! `events.jsonl` is the source of truth. This module projects that log back
//! into an [`ExecutionState`] so a damaged or missing `state.json` can be
//! recovered without guessing from CLI output.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use shipper_types::{
    AttemptDetail, ErrorClass, EventType, ExecutionState, PackageProgress, PackageState,
    PublishEvent, ReconciliationOutcome, Registry,
};

use super::{events, execution_state, upload_evidence::UploadWitness};

/// Inputs that cannot be recovered from `events.jsonl` alone.
#[derive(Debug, Clone)]
pub struct StateRebuildOptions {
    pub registry: Registry,
    pub fallback_plan_id: Option<String>,
}

impl StateRebuildOptions {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            fallback_plan_id: None,
        }
    }

    pub fn with_fallback_plan_id(mut self, plan_id: impl Into<String>) -> Self {
        self.fallback_plan_id = Some(plan_id.into());
        self
    }
}

/// Project an [`ExecutionState`] from an event log.
///
/// The registry is supplied by the caller because publish events currently
/// record the plan id but not the full registry definition. If the log contains
/// no `plan_created` event, `fallback_plan_id` is used; otherwise this returns
/// an error.
///
/// Replay follows JSONL append order, never timestamp order. For a nonempty
/// log, `created_at` and `updated_at` are the first and last appended events'
/// timestamps, not the minimum and maximum wall-clock values. Both use the
/// rebuild time for an empty log. Attempt timestamps follow the existing
/// per-event completion semantics without clock normalization. A `StillUnknown`
/// reconciliation can replace an earlier failure end with its event time, so
/// live and rebuilt attempt ends are not universally identical. Clock rollback
/// can leave an attempt's end earlier than its start.
pub fn rebuild_state_from_events(
    events_path: &Path,
    options: StateRebuildOptions,
) -> Result<ExecutionState> {
    let log = events::EventLog::read_from_file(events_path).with_context(|| {
        format!(
            "failed to read event log for state rebuild: {}",
            events_path.display()
        )
    })?;
    let events = log.all_events();
    let now = Utc::now();
    let created_at = events.first().map(|event| event.timestamp).unwrap_or(now);
    let updated_at = events.last().map(|event| event.timestamp).unwrap_or(now);
    let mut plan_id = options.fallback_plan_id;
    let mut packages = BTreeMap::new();
    let mut attempt_history = Vec::new();
    let mut active_attempts: BTreeMap<String, RebuildAttemptDetail> = BTreeMap::new();

    let mut upload_witnesses = BTreeMap::<&str, UploadWitness>::new();
    for event in events {
        let witness = upload_witnesses.entry(&event.package).or_default();
        let previous = *witness;
        *witness = witness.observe(&event.event_type);
        apply_event(
            event,
            (previous, *witness),
            &mut plan_id,
            &mut packages,
            &mut active_attempts,
            &mut attempt_history,
        );
    }
    finalize_active_attempts(&mut active_attempts, &mut attempt_history);

    let Some(plan_id) = plan_id else {
        bail!(
            "cannot rebuild state from {}: no plan_created event and no fallback plan_id supplied",
            events_path.display()
        );
    };

    Ok(ExecutionState {
        state_version: execution_state::CURRENT_STATE_VERSION.to_string(),
        plan_id,
        registry: options.registry,
        created_at,
        updated_at,
        attempt_history,
        packages,
    })
}

/// Rebuild and write `<state_dir>/state.json` from `<state_dir>/events.jsonl`.
///
/// This low-level writer requires an offline state directory or exclusive
/// caller ownership of the same publication key for the whole call. It does
/// not acquire an invocation lock: this API has no workspace/ownership input.
/// Concurrent publication, cleanup, or arbitrary embedding writes are not
/// supported. Under that precondition, an already-retained projection-only
/// cleanup hold cannot be erased by rebuilding an older event prefix.
pub fn rebuild_state_file_from_events(
    state_dir: &Path,
    options: StateRebuildOptions,
) -> Result<ExecutionState> {
    let events_path = events::events_path(state_dir);
    let state = rebuild_state_from_events(&events_path, options)?;
    // A cleanup hold saved only in the projection is evidence of an incomplete
    // persistence boundary. Rebuilding from an older event prefix must not
    // silently erase that observed local-safety condition. The caller provides
    // exclusion against concurrent writers as required by this API contract.
    let retained = match execution_state::load_state(state_dir) {
        Ok(retained) => retained,
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
            // A damaged projection is the reason this recovery API exists.
            // Still preserve a complete typed hold nested in otherwise invalid
            // JSON/schema; never mistake a projection parse failure for proof
            // that local process cleanup was confirmed.
            let raw = std::fs::read(execution_state::state_path(state_dir))
                .context("failed to inspect damaged state before rebuilding")?;
            if recognizable_cleanup_hold(&raw) {
                bail!(
                    "cannot rebuild away retained cleanup hold in damaged state; preserve the evidence and inspect the owned processes"
                );
            }
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(retained) = retained {
        for (key, progress) in &retained.packages {
            if matches!(progress.state, PackageState::CleanupIncomplete { .. })
                && !state.packages.get(key).is_some_and(|rebuilt| {
                    matches!(rebuilt.state, PackageState::CleanupIncomplete { .. })
                })
            {
                bail!(
                    "cannot rebuild away retained cleanup hold for {key}: its authoritative event is missing; preserve the evidence and inspect the owned processes"
                );
            }
        }
    }
    execution_state::save_state(state_dir, &state)?;
    Ok(state)
}

/// Find complete typed states even when surrounding projection JSON is damaged.
/// Deserialize each possible object boundary as a prefix so trailing damage
/// cannot erase an already recognizable hold. Escaped strings are handled by
/// serde, and arbitrary prose mentioning the state name is not a typed hold.
fn recognizable_cleanup_hold(raw: &[u8]) -> bool {
    raw.iter().enumerate().any(|(offset, byte)| {
        *byte == b'{'
            && matches!(
                serde_json::Deserializer::from_slice(&raw[offset..])
                    .into_iter::<PackageState>()
                    .next(),
                Some(Ok(PackageState::CleanupIncomplete { .. }))
            )
    })
}

fn apply_event(
    event: &PublishEvent,
    upload_transition: (UploadWitness, UploadWitness),
    plan_id: &mut Option<String>,
    packages: &mut BTreeMap<String, PackageProgress>,
    active_attempts: &mut BTreeMap<String, RebuildAttemptDetail>,
    attempt_history: &mut Vec<AttemptDetail>,
) {
    let affected_key = match &event.event_type {
        EventType::PackageStarted { name, version } => format!("{name}@{version}"),
        _ => event.package.clone(),
    };
    let retained_hold = packages.get(&affected_key).and_then(|progress| {
        matches!(progress.state, PackageState::CleanupIncomplete { .. })
            .then(|| progress.state.clone())
    });
    match &event.event_type {
        EventType::PlanCreated {
            plan_id: event_plan_id,
            ..
        } => {
            *plan_id = Some(event_plan_id.clone());
        }
        EventType::PackageStarted { name, version } => {
            let key = format!("{}@{}", name, version);
            let progress = ensure_package(packages, &key, name, version, event.timestamp);
            if !matches!(
                progress.state,
                PackageState::Uploaded
                    | PackageState::Ambiguous { .. }
                    | PackageState::Failed {
                        class: ErrorClass::Ambiguous,
                        ..
                    }
            ) && !matches!(upload_transition.1, UploadWitness::NotLaunched(_))
            {
                progress.state = PackageState::Pending;
            }
            // The new invocation may reconcile an old upload. Its result
            // must not rewrite the earlier Cargo attempt's durable outcome.
            if active_attempts.get(&key).is_some_and(|active| {
                active.upload_checkpoint == Some(UploadCheckpoint::Provisional)
            }) {
                active_attempts.remove(&key);
            }
            progress.last_updated_at = event.timestamp;
        }
        EventType::ReadinessStarted { .. } => {
            // v0.4 had no PackageUploaded marker. Its first readiness check
            // followed the successful Cargo attempt and the Uploaded save.
            // A modern readiness start can be only a visibility probe.
            if let (UploadWitness::Attempted { .. }, UploadWitness::LegacyUploaded(attempt)) =
                upload_transition
                && let Some(active) = active_attempt_for_key_mut(active_attempts, &event.package)
                && active.attempt == attempt
            {
                active.ended_at = event.timestamp;
                finalize_attempt(active_attempts, attempt_history, &event.package);
                if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                    progress.state = PackageState::Uploaded;
                    progress.last_updated_at = event.timestamp;
                }
            }
        }
        EventType::PackageUploaded => {
            // Live publish commits the matching attempt detail at the Uploaded
            // boundary as a success (no error_class). Finalize here so later
            // readiness-timeout Retry* events cannot rewrite that attempt.
            if let Some(active) = active_attempt_for_key_mut(active_attempts, &event.package) {
                active.ended_at = event.timestamp;
                if active.error_class.is_none() {
                    finalize_attempt(active_attempts, attempt_history, &event.package);
                }
            }
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = PackageState::Uploaded;
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackageAttempted {
            attempt,
            max_attempts,
            ..
        } => {
            if let Some((name, version)) = split_package_label(&event.package) {
                start_rebuild_attempt(
                    active_attempts,
                    attempt_history,
                    &event.package,
                    name,
                    version,
                    *attempt,
                    *max_attempts,
                    event.timestamp,
                );
            }
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.attempts = progress.attempts.max(*attempt);
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackagePublished { .. } => {
            if let Some(active) = active_attempt_for_key_mut(active_attempts, &event.package)
                && active.error_class.is_none()
                && active.ended_at == active.started_at
            {
                active.ended_at = event.timestamp;
            }
            finalize_attempt(active_attempts, attempt_history, &event.package);
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = PackageState::Published;
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackageSkipped { reason } => {
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = PackageState::Skipped {
                    reason: reason.clone(),
                };
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackageNotLaunched { message, .. }
            if matches!(
                upload_transition,
                (
                    UploadWitness::Attempted { .. },
                    UploadWitness::NotLaunched(_)
                )
            ) =>
        {
            if let Some(active) = active_attempt_for_key_mut(active_attempts, &event.package) {
                active.ended_at = event.timestamp;
                active.error_class = Some(ErrorClass::Retryable);
                active.redacted_message = Some(message.clone());
            }
            finalize_attempt(active_attempts, attempt_history, &event.package);
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: message.clone(),
                };
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackageUploadUnconfirmed { class, message } => {
            if let Some(active) = active_attempt_for_key_mut(active_attempts, &event.package) {
                active.ended_at = event.timestamp;
                active.error_class = Some(class.clone());
                active.redacted_message = Some(message.clone());
                active.upload_checkpoint = Some(UploadCheckpoint::Provisional);
                attempt_history.push(rebuild_attempt_to_detail(active.clone()));
            }
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = PackageState::Ambiguous {
                    message: message.clone(),
                };
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackageFailed { class, message } => {
            if let Some((name, version)) = split_package_label(&event.package) {
                apply_package_failed(
                    active_attempts,
                    attempt_history,
                    &event.package,
                    name,
                    version,
                    event.timestamp,
                    class,
                    message,
                );
            }
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                // Preserve the explicit PackageFailed state, including an
                // ambiguous failure that was safely retried after a
                // NotPublished reconciliation. A StillUnknown outcome has
                // its own PublishReconciled event and projects to Ambiguous
                // below, so rebuild does not need to infer that state here.
                progress.state = PackageState::Failed {
                    class: class.clone(),
                    message: message.clone(),
                };
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackageCleanupIncomplete { message } => {
            if let Some(active) = active_attempt_for_key_mut(active_attempts, &event.package) {
                active.ended_at = event.timestamp;
                active.error_class = Some(ErrorClass::Ambiguous);
                active.next_attempt_at = None;
                active.redacted_message = Some(message.clone());
            }
            finalize_attempt(active_attempts, attempt_history, &event.package);
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = PackageState::CleanupIncomplete {
                    message: message.clone(),
                };
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PublishReconciled { outcome } => {
            match outcome {
                ReconciliationOutcome::StillUnknown { reason, .. } => {
                    if let Some(active) =
                        active_attempt_for_key_mut(active_attempts, &event.package)
                    {
                        if active.upload_checkpoint.is_none() {
                            // Older journals use reconciliation as the
                            // attempt's completion boundary. A modern
                            // checkpoint already recorded the Cargo result.
                            active.ended_at = event.timestamp;
                            active.error_class = Some(ErrorClass::Ambiguous);
                            if active.redacted_message.is_none() {
                                active.redacted_message = Some(reason.clone());
                            }
                        }
                        finalize_attempt(active_attempts, attempt_history, &event.package);
                    }
                }
                ReconciliationOutcome::NotPublished { .. }
                | ReconciliationOutcome::Published { .. } => {}
            }
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = match outcome {
                    ReconciliationOutcome::Published { .. } => PackageState::Published,
                    ReconciliationOutcome::NotPublished { .. } => PackageState::Pending,
                    ReconciliationOutcome::StillUnknown { reason, .. } => PackageState::Ambiguous {
                        message: reason.clone(),
                    },
                };
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::RetryBackoffStarted {
            attempt,
            max_attempts,
            next_attempt_at,
            reason,
            message,
            ..
        }
        | EventType::RetryScheduled {
            attempt,
            max_attempts,
            next_attempt_at,
            reason,
            message,
            ..
        } => {
            if let Some(active) = active_attempt_for_key_mut(active_attempts, &event.package)
                && active.attempt == *attempt
            {
                apply_retry_wait(
                    active,
                    *max_attempts,
                    *next_attempt_at,
                    reason,
                    message,
                    event.timestamp,
                );
                // A checkpointed ordinary failure commits Pending and the
                // retry schedule together after confirming registry absence.
                // Keep the older journal projection for pre-checkpoint logs.
                if active.upload_checkpoint == Some(UploadCheckpoint::Classified)
                    && matches!(reason, ErrorClass::Retryable)
                    && let Some(progress) = packages.get_mut(&event.package)
                    && matches!(
                        progress.state,
                        PackageState::Failed {
                            class: ErrorClass::Retryable,
                            ..
                        }
                    )
                {
                    progress.state = PackageState::Pending;
                    progress.last_updated_at = event.timestamp;
                }
                finalize_attempt(active_attempts, attempt_history, &event.package);
            }
        }
        _ => {}
    }
    // Keep later observed metadata/attempts, but ordinary success, retry or
    // registry events cannot resolve a local cleanup hold.
    if let Some(held) = retained_hold
        && let Some(progress) = packages.get_mut(&affected_key)
    {
        progress.state = held;
    }
}

#[derive(Debug, Clone)]
struct RebuildAttemptDetail {
    package: String,
    version: String,
    attempt: u32,
    max_attempts: u32,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    error_class: Option<ErrorClass>,
    next_attempt_at: Option<DateTime<Utc>>,
    redacted_message: Option<String>,
    saw_failure: bool,
    upload_checkpoint: Option<UploadCheckpoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadCheckpoint {
    Provisional,
    Classified,
}

fn start_rebuild_attempt(
    active_attempts: &mut BTreeMap<String, RebuildAttemptDetail>,
    attempt_history: &mut Vec<AttemptDetail>,
    package_key: &str,
    name: &str,
    version: &str,
    attempt: u32,
    max_attempts: u32,
    timestamp: DateTime<Utc>,
) {
    let max_attempts = if max_attempts == 0 {
        attempt
    } else {
        max_attempts
    };
    // A resumed retry can begin while the preceding checkpoint is still
    // active in replay. Close it through the checkpoint-aware path so an
    // already recorded attempt is updated in place, not appended again.
    finalize_attempt(active_attempts, attempt_history, package_key);
    active_attempts.insert(
        package_key.to_string(),
        RebuildAttemptDetail {
            package: name.to_string(),
            version: version.to_string(),
            attempt,
            max_attempts,
            started_at: timestamp,
            ended_at: timestamp,
            error_class: None,
            next_attempt_at: None,
            redacted_message: None,
            saw_failure: false,
            upload_checkpoint: None,
        },
    );
}

fn apply_package_failed(
    active_attempts: &mut BTreeMap<String, RebuildAttemptDetail>,
    attempt_history: &mut Vec<AttemptDetail>,
    package_key: &str,
    _name: &str,
    _version: &str,
    timestamp: DateTime<Utc>,
    class: &ErrorClass,
    message: &str,
) {
    let Some(active) = active_attempt_for_key_mut(active_attempts, package_key) else {
        return;
    };
    let checkpointed = active.upload_checkpoint == Some(UploadCheckpoint::Provisional);
    if checkpointed {
        active.upload_checkpoint = Some(UploadCheckpoint::Classified);
    }
    // Live attempt details capture the first failure's timestamp/message when the
    // attempt is recorded (retry schedule or exhaustion). A later terminal
    // PackageFailed must not rewrite those fields or finalization drifts.
    if !active.saw_failure && !checkpointed {
        active.ended_at = timestamp;
        active.error_class = Some(class.clone());
        active.redacted_message = Some(message.to_string());
    }
    if matches!(class, ErrorClass::Permanent) || active.saw_failure {
        finalize_attempt(active_attempts, attempt_history, package_key);
    } else {
        active.saw_failure = true;
    }
}

fn apply_retry_wait(
    active: &mut RebuildAttemptDetail,
    max_attempts: u32,
    next_attempt_at: DateTime<Utc>,
    reason: &ErrorClass,
    message: &str,
    _timestamp: DateTime<Utc>,
) {
    active.max_attempts = max_attempts;
    active.next_attempt_at = Some(next_attempt_at);
    if active.error_class.is_none() {
        active.error_class = Some(reason.clone());
    }
    if active.redacted_message.is_none() {
        active.redacted_message = Some(message.to_string());
    }
}

fn finalize_attempt(
    active_attempts: &mut BTreeMap<String, RebuildAttemptDetail>,
    attempt_history: &mut Vec<AttemptDetail>,
    package_key: &str,
) {
    if let Some(active) = active_attempts.remove(package_key)
        && active.upload_checkpoint != Some(UploadCheckpoint::Provisional)
    {
        let checkpointed = active.upload_checkpoint.is_some();
        let mut attempt = rebuild_attempt_to_detail(active);
        if attempt.max_attempts < attempt.attempt {
            attempt.max_attempts = attempt.attempt;
        }
        record_rebuilt_detail(attempt_history, attempt, checkpointed);
    }
}

fn finalize_active_attempts(
    active_attempts: &mut BTreeMap<String, RebuildAttemptDetail>,
    attempt_history: &mut Vec<AttemptDetail>,
) {
    let mut active = Vec::with_capacity(active_attempts.len());
    while let Some((_, active_attempt)) = active_attempts.pop_first() {
        if active_attempt.upload_checkpoint != Some(UploadCheckpoint::Provisional) {
            active.push(active_attempt);
        }
    }
    active.sort_by(|left, right| {
        left.started_at
            .cmp(&right.started_at)
            .then_with(|| left.package.cmp(&right.package))
    });
    for active_attempt in active {
        let checkpointed = active_attempt.upload_checkpoint.is_some();
        let detail = rebuild_attempt_to_detail(active_attempt);
        record_rebuilt_detail(attempt_history, detail, checkpointed);
    }
}

fn record_rebuilt_detail(
    history: &mut Vec<AttemptDetail>,
    detail: AttemptDetail,
    checkpointed: bool,
) {
    if checkpointed {
        super::record_attempt_detail(history, detail);
    } else {
        history.push(detail);
    }
}

fn active_attempt_for_key_mut<'a>(
    active_attempts: &'a mut BTreeMap<String, RebuildAttemptDetail>,
    key: &str,
) -> Option<&'a mut RebuildAttemptDetail> {
    active_attempts.get_mut(key)
}

fn rebuild_attempt_to_detail(active: RebuildAttemptDetail) -> AttemptDetail {
    AttemptDetail {
        package: active.package,
        version: active.version,
        attempt: active.attempt,
        max_attempts: active.max_attempts,
        started_at: active.started_at,
        ended_at: active.ended_at,
        error_class: active.error_class,
        next_attempt_at: active.next_attempt_at,
        redacted_message: active.redacted_message,
    }
}

fn ensure_event_package<'a>(
    packages: &'a mut BTreeMap<String, PackageProgress>,
    event: &PublishEvent,
    timestamp: DateTime<Utc>,
) -> Option<&'a mut PackageProgress> {
    split_package_label(&event.package)
        .map(|(name, version)| ensure_package(packages, &event.package, name, version, timestamp))
}

fn ensure_package<'a>(
    packages: &'a mut BTreeMap<String, PackageProgress>,
    key: &str,
    name: &str,
    version: &str,
    timestamp: DateTime<Utc>,
) -> &'a mut PackageProgress {
    packages
        .entry(key.to_string())
        .or_insert_with(|| PackageProgress {
            name: name.to_string(),
            version: version.to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: timestamp,
        })
}

fn split_package_label(package: &str) -> Option<(&str, &str)> {
    if package == "all" || package.is_empty() {
        return None;
    }
    package.rsplit_once('@')
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use shipper_types::{ErrorClass, ReadinessMethod, ReconciliationOutcome};
    use tempfile::tempdir;

    use super::*;

    fn ts(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, second)
            .single()
            .expect("valid timestamp")
    }

    fn event(second: u32, package: &str, event_type: EventType) -> PublishEvent {
        PublishEvent {
            timestamp: ts(second),
            event_type,
            package: package.to_string(),
        }
    }

    fn options() -> StateRebuildOptions {
        StateRebuildOptions::new(Registry::crates_io())
    }

    fn write_events(path: &Path, events: Vec<PublishEvent>) {
        let mut log = events::EventLog::new();
        for event in events {
            log.record(event);
        }
        log.write_to_file(path).expect("write events");
    }

    #[test]
    #[serial_test::serial]
    fn rebuild_append_order_preserves_original_attempt_times_and_cleanup_hold() {
        let td = tempdir().expect("tempdir");
        let events_path = events::events_path(td.path());
        // Worker b observed these events earlier, but appends after worker a.
        let delayed = vec![
            event(
                20,
                "b@1.0.0",
                EventType::PackageAttempted {
                    attempt: 1,
                    command: "cargo publish".to_string(),
                    max_attempts: 1,
                },
            ),
            event(21, "b@1.0.0", EventType::PackageUploaded),
            event(
                22,
                "b@1.0.0",
                EventType::PackagePublished { duration_ms: 2 },
            ),
        ];
        write_events(
            &events_path,
            vec![
                event(
                    30,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "append-order-plan".to_string(),
                        package_count: 3,
                    },
                ),
                event(
                    50,
                    "a@1.0.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 2,
                    },
                ),
            ],
        );
        write_events(&events_path, delayed);
        write_events(
            &events_path,
            vec![
                // Wall-clock rollback does not reverse a's causal sequence:
                // failure, retry decision, second attempt, then upload.
                event(
                    42,
                    "a@1.0.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "rate limited".to_string(),
                    },
                ),
                event(
                    43,
                    "a@1.0.0",
                    EventType::RetryScheduled {
                        attempt: 1,
                        max_attempts: 2,
                        delay_ms: 1_000,
                        next_attempt_at: ts(44),
                        reason: ErrorClass::Retryable,
                        message: "retry after backoff".to_string(),
                    },
                ),
                event(
                    44,
                    "a@1.0.0",
                    EventType::PackageAttempted {
                        attempt: 2,
                        command: "cargo publish".to_string(),
                        max_attempts: 2,
                    },
                ),
                event(45, "a@1.0.0", EventType::PackageUploaded),
                event(
                    46,
                    "a@1.0.0",
                    EventType::PackagePublished { duration_ms: 2 },
                ),
                event(
                    58,
                    "held@1.0.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(
                    15,
                    "held@1.0.0",
                    EventType::PackageCleanupIncomplete {
                        message: "cleanup could not be confirmed".to_string(),
                    },
                ),
                // A later ordinary event updates observed metadata but cannot
                // clear a retained hold after clock rollback.
                event(
                    16,
                    "held@1.0.0",
                    EventType::PackagePublished { duration_ms: 1 },
                ),
            ],
        );
        let original_bytes = std::fs::read(&events_path).expect("original events");

        let state = rebuild_state_file_from_events(td.path(), options()).expect("rebuild file");
        assert_eq!(state.plan_id, "append-order-plan");
        assert_eq!(state.created_at, ts(30), "first append, not minimum time");
        assert_eq!(state.updated_at, ts(16), "last append, not maximum time");
        assert!(state.updated_at < state.created_at);
        assert_eq!(state.packages.len(), 3);
        for (key, attempts, last_update) in [("a@1.0.0", 2, 46), ("b@1.0.0", 1, 22)] {
            let package = &state.packages[key];
            assert!(matches!(package.state, PackageState::Published));
            assert_eq!(package.attempts, attempts);
            assert_eq!(package.last_updated_at, ts(last_update));
        }
        let held = &state.packages["held@1.0.0"];
        assert!(matches!(
            &held.state,
            PackageState::CleanupIncomplete { message }
                if message == "cleanup could not be confirmed"
        ));
        assert_eq!(held.attempts, 1);
        assert_eq!(held.last_updated_at, ts(16));

        // These are completed attempts in replay/finalization order. Sorting
        // by start time would reverse a's attempts; clamping would fabricate
        // the first failure and cleanup observations.
        assert_eq!(
            state.attempt_history,
            vec![
                AttemptDetail {
                    package: "b".to_string(),
                    version: "1.0.0".to_string(),
                    attempt: 1,
                    max_attempts: 1,
                    started_at: ts(20),
                    ended_at: ts(21),
                    error_class: None,
                    next_attempt_at: None,
                    redacted_message: None,
                },
                AttemptDetail {
                    package: "a".to_string(),
                    version: "1.0.0".to_string(),
                    attempt: 1,
                    max_attempts: 2,
                    started_at: ts(50),
                    ended_at: ts(42),
                    error_class: Some(ErrorClass::Retryable),
                    next_attempt_at: Some(ts(44)),
                    redacted_message: Some("rate limited".to_string()),
                },
                AttemptDetail {
                    package: "a".to_string(),
                    version: "1.0.0".to_string(),
                    attempt: 2,
                    max_attempts: 2,
                    started_at: ts(44),
                    ended_at: ts(45),
                    error_class: None,
                    next_attempt_at: None,
                    redacted_message: None,
                },
                AttemptDetail {
                    package: "held".to_string(),
                    version: "1.0.0".to_string(),
                    attempt: 1,
                    max_attempts: 1,
                    started_at: ts(58),
                    ended_at: ts(15),
                    error_class: Some(ErrorClass::Ambiguous),
                    next_attempt_at: None,
                    redacted_message: Some("cleanup could not be confirmed".to_string()),
                },
            ]
        );
        let persisted = execution_state::load_state(td.path())
            .expect("load rebuilt file")
            .expect("persisted state");
        assert_eq!(
            serde_json::to_value(&persisted).expect("persisted state JSON"),
            serde_json::to_value(&state).expect("rebuilt state JSON")
        );
        assert_eq!(
            std::fs::read(&events_path).expect("events after rebuild"),
            original_bytes,
            "rebuild must not rewrite authoritative observations"
        );
    }

    #[test]
    fn rebuild_missing_events_uses_fallback_plan_id() {
        let td = tempdir().expect("tempdir");

        let state = rebuild_state_from_events(
            &td.path().join("events.jsonl"),
            options().with_fallback_plan_id("fallback-plan"),
        )
        .expect("rebuild");

        assert_eq!(state.plan_id, "fallback-plan");
        assert!(state.packages.is_empty());
        assert!(state.attempt_history.is_empty());
    }

    #[test]
    fn rebuild_requires_plan_id_source() {
        let td = tempdir().expect("tempdir");

        let err = rebuild_state_from_events(&td.path().join("events.jsonl"), options())
            .expect_err("missing plan id should fail");

        assert!(err.to_string().contains("no plan_created event"));
    }

    #[test]
    fn rebuild_package_started_creates_pending_progress() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageStarted {
                        name: "demo".to_string(),
                        version: "0.1.0".to_string(),
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let progress = state.packages.get("demo@0.1.0").expect("package");

        assert_eq!(state.plan_id, "plan-123");
        assert_eq!(progress.name, "demo");
        assert_eq!(progress.version, "0.1.0");
        assert_eq!(progress.attempts, 0);
        assert_eq!(progress.state, PackageState::Pending);
        assert_eq!(progress.last_updated_at, ts(1));
    }

    #[test]
    fn rebuild_attempted_updates_attempt_count() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(
                    2,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 3,
                        command: "cargo publish".to_string(),
                        max_attempts: 3,
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let progress = state.packages.get("demo@0.1.0").expect("package");

        assert_eq!(progress.attempts, 3);
        assert_eq!(progress.state, PackageState::Pending);
        assert_eq!(progress.last_updated_at, ts(2));
    }

    #[test]
    fn rebuild_reconstructs_success_attempt_history() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(2, "demo@0.1.0", EventType::PackageUploaded),
                event(
                    3,
                    "demo@0.1.0",
                    EventType::PackagePublished { duration_ms: 10 },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let history = &state.attempt_history;

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].package, "demo");
        assert_eq!(history[0].version, "0.1.0");
        assert_eq!(history[0].attempt, 1);
        assert_eq!(history[0].max_attempts, 1);
        assert_eq!(history[0].started_at, ts(1));
        assert_eq!(history[0].ended_at, ts(2));
        assert!(history[0].error_class.is_none());
        assert!(history[0].redacted_message.is_none());
        assert!(history[0].next_attempt_at.is_none());
    }

    #[test]
    fn rebuild_reconstructs_retry_attempts_with_backoff_timestamps() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        let next_attempt_at = ts(4);
        let second_next_attempt_at = ts(8);
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 3,
                    },
                ),
                event(
                    2,
                    "demo@0.1.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "rate limited".to_string(),
                    },
                ),
                event(
                    3,
                    "demo@0.1.0",
                    EventType::RetryScheduled {
                        attempt: 1,
                        max_attempts: 3,
                        delay_ms: 1_000,
                        next_attempt_at,
                        reason: ErrorClass::Retryable,
                        message: "retry after backoff".to_string(),
                    },
                ),
                event(
                    5,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 2,
                        command: "cargo publish".to_string(),
                        max_attempts: 3,
                    },
                ),
                event(
                    6,
                    "demo@0.1.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "network glitch".to_string(),
                    },
                ),
                event(
                    7,
                    "demo@0.1.0",
                    EventType::RetryBackoffStarted {
                        attempt: 2,
                        max_attempts: 3,
                        delay_ms: 2_000,
                        next_attempt_at: second_next_attempt_at,
                        reason: ErrorClass::Retryable,
                        message: "wait again".to_string(),
                    },
                ),
                event(
                    8,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 3,
                        command: "cargo publish".to_string(),
                        max_attempts: 3,
                    },
                ),
                event(9, "demo@0.1.0", EventType::PackageUploaded),
                event(
                    10,
                    "demo@0.1.0",
                    EventType::PackagePublished { duration_ms: 11 },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let history = &state.attempt_history;

        assert_eq!(history.len(), 3);
        assert_eq!(history[0].attempt, 1);
        assert_eq!(history[0].max_attempts, 3);
        assert_eq!(history[0].error_class, Some(ErrorClass::Retryable));
        assert_eq!(history[0].redacted_message.as_deref(), Some("rate limited"));
        assert_eq!(history[0].next_attempt_at, Some(next_attempt_at));

        assert_eq!(history[1].attempt, 2);
        assert_eq!(history[1].max_attempts, 3);
        assert_eq!(history[1].error_class, Some(ErrorClass::Retryable));
        assert_eq!(
            history[1].redacted_message.as_deref(),
            Some("network glitch")
        );
        assert_eq!(history[1].next_attempt_at, Some(second_next_attempt_at));

        assert_eq!(history[2].attempt, 3);
        assert_eq!(history[2].max_attempts, 3);
        assert!(history[2].error_class.is_none());
        assert!(history[2].next_attempt_at.is_none());
        assert_eq!(history[2].ended_at, ts(9));
    }

    #[test]
    fn rebuild_interleaves_attempts_for_multiple_packages_without_cross_bleed() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 2,
                    },
                ),
                event(
                    1,
                    "alpha@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 3,
                    },
                ),
                event(
                    2,
                    "alpha@0.1.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "rate limited".to_string(),
                    },
                ),
                event(
                    3,
                    "alpha@0.1.0",
                    EventType::RetryScheduled {
                        attempt: 1,
                        max_attempts: 3,
                        delay_ms: 1_000,
                        next_attempt_at: ts(10),
                        reason: ErrorClass::Retryable,
                        message: "alpha retry".to_string(),
                    },
                ),
                event(
                    4,
                    "beta@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(5, "beta@0.1.0", EventType::PackageUploaded),
                event(
                    6,
                    "beta@0.1.0",
                    EventType::PackagePublished { duration_ms: 7 },
                ),
                event(
                    7,
                    "alpha@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 2,
                        command: "cargo publish".to_string(),
                        max_attempts: 3,
                    },
                ),
                event(8, "alpha@0.1.0", EventType::PackageUploaded),
                event(
                    9,
                    "alpha@0.1.0",
                    EventType::PackagePublished { duration_ms: 9 },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let history = &state.attempt_history;

        assert_eq!(history.len(), 3);

        assert_eq!(history[0].package, "alpha");
        assert_eq!(history[0].version, "0.1.0");
        assert_eq!(history[0].attempt, 1);
        assert_eq!(history[0].error_class, Some(ErrorClass::Retryable));
        assert_eq!(history[0].redacted_message.as_deref(), Some("rate limited"));
        assert_eq!(history[0].next_attempt_at, Some(ts(10)));
        assert_eq!(history[0].ended_at, ts(2));

        assert_eq!(history[1].package, "beta");
        assert_eq!(history[1].version, "0.1.0");
        assert_eq!(history[1].attempt, 1);
        assert!(history[1].error_class.is_none());
        assert_eq!(history[1].ended_at, ts(5));

        assert_eq!(history[2].package, "alpha");
        assert_eq!(history[2].version, "0.1.0");
        assert_eq!(history[2].attempt, 2);
        assert!(history[2].error_class.is_none());
        assert!(history[2].next_attempt_at.is_none());
        assert_eq!(history[2].ended_at, ts(8));
    }

    #[test]
    fn rebuild_preserves_tail_attempt_order_across_packages() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 2,
                    },
                ),
                event(
                    1,
                    "zeta@1.0.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(
                    2,
                    "alpha@1.0.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let history = &state.attempt_history;

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].package, "zeta");
        assert_eq!(history[1].package, "alpha");
    }

    #[test]
    fn rebuild_preserves_ambiguous_reconciliation_success_path_in_history() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(
                    2,
                    "demo@0.1.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Ambiguous,
                        message: "cargo ambiguous".to_string(),
                    },
                ),
                event(
                    3,
                    "demo@0.1.0",
                    EventType::PublishReconciling {
                        method: ReadinessMethod::Api,
                    },
                ),
                event(
                    4,
                    "demo@0.1.0",
                    EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::Published {
                            attempts: 1,
                            elapsed_ms: 10,
                        },
                    },
                ),
                event(
                    5,
                    "demo@0.1.0",
                    EventType::PackagePublished { duration_ms: 10 },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let history = &state.attempt_history;

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].attempt, 1);
        assert_eq!(history[0].max_attempts, 1);
        assert_eq!(history[0].error_class, Some(ErrorClass::Ambiguous));
        assert_eq!(
            history[0].redacted_message.as_deref(),
            Some("cargo ambiguous")
        );
    }

    #[test]
    fn rebuild_deduplicates_duplicate_terminal_failed_events() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(
                    2,
                    "demo@0.1.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "first failure".to_string(),
                    },
                ),
                event(
                    3,
                    "demo@0.1.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "final failure".to_string(),
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");

        // Duplicate terminal failures collapse to one attempt detail. Live state
        // records the first failure's message/timestamp when the attempt is
        // persisted, so rebuild must not let a later PackageFailed rewrite it.
        assert_eq!(state.attempt_history.len(), 1);
        assert_eq!(
            state.attempt_history[0].redacted_message.as_deref(),
            Some("first failure")
        );
    }

    #[test]
    fn rebuild_reconstructs_permanent_failure_attempt_as_terminal() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "frozen@0.2.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".to_string(),
                        max_attempts: 1,
                    },
                ),
                event(
                    2,
                    "frozen@0.2.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Permanent,
                        message: "auth denied".to_string(),
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        assert_eq!(
            state.packages["frozen@0.2.0"].state,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "auth denied".to_string()
            }
        );
        assert_eq!(state.attempt_history.len(), 1);
        assert_eq!(state.attempt_history[0].package, "frozen");
        assert_eq!(state.attempt_history[0].version, "0.2.0");
        assert_eq!(state.attempt_history[0].attempt, 1);
        assert_eq!(
            state.attempt_history[0].error_class,
            Some(ErrorClass::Permanent)
        );
        assert_eq!(
            state.attempt_history[0].redacted_message.as_deref(),
            Some("auth denied")
        );
        assert_eq!(state.attempt_history[0].ended_at, ts(2));
        assert!(state.attempt_history[0].next_attempt_at.is_none());
    }

    #[test]
    fn rebuild_package_uploaded_projects_uploaded_until_published() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(1, "demo@0.1.0", EventType::PackageUploaded),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let progress = state.packages.get("demo@0.1.0").expect("package");
        assert_eq!(progress.state, PackageState::Uploaded);
        assert_eq!(progress.last_updated_at, ts(1));

        write_events(
            &events_path,
            vec![event(
                2,
                "demo@0.1.0",
                EventType::PackagePublished { duration_ms: 10 },
            )],
        );
        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        assert_eq!(state.packages["demo@0.1.0"].state, PackageState::Published);
    }

    #[test]
    fn rebuild_interrupt_resumes_from_uploaded_checkpoint() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 2,
                    },
                ),
                event(1, "core@0.1.0", EventType::PackageUploaded),
                event(2, "app@0.2.0", EventType::PackageUploaded),
                event(
                    3,
                    "app@0.2.0",
                    EventType::PackagePublished { duration_ms: 11 },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let core = state.packages.get("core@0.1.0").expect("package");
        let app = state.packages.get("app@0.2.0").expect("package");

        assert_eq!(core.state, PackageState::Uploaded);
        assert_eq!(core.last_updated_at, ts(1));
        assert_eq!(app.state, PackageState::Published);
        assert_eq!(app.last_updated_at, ts(3));
    }

    #[test]
    fn rebuild_rejects_corrupt_event_log() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        std::fs::write(&events_path, "invalid-json-line").expect("write corrupt");

        let err = rebuild_state_from_events(&events_path, options())
            .expect_err("corrupt log should fail");

        assert!(err.to_string().contains("failed to read event log"));
    }

    #[test]
    fn rebuild_recovers_legacy_acceptance_from_the_first_readiness_start() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish".into(),
                        max_attempts: 0,
                    },
                ),
                event(
                    2,
                    "demo@0.1.0",
                    EventType::PackageOutput {
                        stdout_tail: "Uploading demo".into(),
                        stderr_tail: String::new(),
                    },
                ),
                event(
                    3,
                    "demo@0.1.0",
                    EventType::ReadinessStarted {
                        method: ReadinessMethod::Api,
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");
        let progress = state.packages.get("demo@0.1.0").expect("package");
        assert_eq!(progress.state, PackageState::Uploaded);
        assert_eq!(progress.last_updated_at, ts(3));
    }

    #[test]
    fn rebuild_terminal_events_project_package_state() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 4,
                    },
                ),
                event(
                    1,
                    "published@1.0.0",
                    EventType::PackagePublished { duration_ms: 10 },
                ),
                event(
                    2,
                    "skipped@1.0.0",
                    EventType::PackageSkipped {
                        reason: "already present".to_string(),
                    },
                ),
                event(
                    3,
                    "failed@1.0.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Permanent,
                        message: "auth failed".to_string(),
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");

        assert_eq!(
            state.packages["published@1.0.0"].state,
            PackageState::Published
        );
        assert_eq!(
            state.packages["skipped@1.0.0"].state,
            PackageState::Skipped {
                reason: "already present".to_string()
            }
        );
        assert_eq!(
            state.packages["failed@1.0.0"].state,
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "auth failed".to_string()
            }
        );
    }

    #[test]
    fn rebuild_reconciliation_outcomes_override_ambiguous_failure() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 3,
                    },
                ),
                event(
                    1,
                    "published@1.0.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Ambiguous,
                        message: "cargo output ambiguous".to_string(),
                    },
                ),
                event(
                    2,
                    "published@1.0.0",
                    EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::Published {
                            attempts: 1,
                            elapsed_ms: 10,
                        },
                    },
                ),
                event(
                    3,
                    "retry@1.0.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Ambiguous,
                        message: "cargo output ambiguous".to_string(),
                    },
                ),
                event(
                    4,
                    "retry@1.0.0",
                    EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::NotPublished {
                            attempts: 1,
                            elapsed_ms: 10,
                        },
                    },
                ),
                event(
                    5,
                    "unknown@1.0.0",
                    EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::StillUnknown {
                            attempts: 1,
                            elapsed_ms: 10,
                            reason: "registry unavailable".to_string(),
                        },
                    },
                ),
                event(
                    6,
                    "exhausted@1.0.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Ambiguous,
                        message: "safe retry exhausted".to_string(),
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");

        assert_eq!(
            state.packages["published@1.0.0"].state,
            PackageState::Published
        );
        assert_eq!(state.packages["retry@1.0.0"].state, PackageState::Pending);
        assert_eq!(
            state.packages["unknown@1.0.0"].state,
            PackageState::Ambiguous {
                message: "registry unavailable".to_string()
            }
        );
        assert_eq!(
            state.packages["exhausted@1.0.0"].state,
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message: "safe retry exhausted".to_string(),
            }
        );
    }

    #[test]
    fn rebuild_preserves_event_order_last_terminal_wins() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "timeout".to_string(),
                    },
                ),
                event(
                    2,
                    "demo@0.1.0",
                    EventType::PackagePublished { duration_ms: 100 },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");

        assert_eq!(state.packages["demo@0.1.0"].state, PackageState::Published);
        assert_eq!(state.updated_at, ts(2));
    }

    #[test]
    fn rebuild_state_file_from_events_writes_state_json() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let events_path = events::events_path(&state_dir);
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(
                    1,
                    "demo@0.1.0",
                    EventType::PackagePublished { duration_ms: 100 },
                ),
            ],
        );

        let rebuilt = rebuild_state_file_from_events(&state_dir, options()).expect("rebuild");
        let loaded = execution_state::load_state(&state_dir)
            .expect("load")
            .expect("state");

        assert_eq!(rebuilt.plan_id, "plan-123");
        assert_eq!(loaded.packages["demo@0.1.0"].state, PackageState::Published);
    }

    #[test]
    #[serial_test::serial]
    fn rebuild_recovers_corrupt_projection_without_recognizable_cleanup_hold() {
        for damaged in [
            "",
            "{",
            "broken JSON",
            "{}",
            r#"{"packages":false}"#,
            r#"{"message":"cleanup_incomplete"}"#,
            r#"{"state":"cleanup_incomplete","message":false}"#,
        ] {
            let td = tempdir().expect("tempdir");
            let events_path = events::events_path(td.path());
            write_events(
                &events_path,
                vec![
                    event(
                        0,
                        "all",
                        EventType::PlanCreated {
                            plan_id: "plan-123".into(),
                            package_count: 1,
                        },
                    ),
                    event(
                        1,
                        "demo@0.1.0",
                        EventType::PackagePublished { duration_ms: 100 },
                    ),
                ],
            );
            let events_before = std::fs::read(&events_path).unwrap();
            std::fs::write(execution_state::state_path(td.path()), damaged).unwrap();

            let rebuilt = rebuild_state_file_from_events(td.path(), options())
                .expect("authoritative events recover a corrupt projection without a hold");

            assert_eq!(
                rebuilt.packages["demo@0.1.0"].state,
                PackageState::Published
            );
            assert_eq!(
                serde_json::to_value(execution_state::load_state(td.path()).unwrap().unwrap())
                    .unwrap(),
                serde_json::to_value(rebuilt).unwrap(),
            );
            assert_eq!(std::fs::read(events_path).unwrap(), events_before);
        }
    }

    #[test]
    #[serial_test::serial]
    fn rebuild_preserves_recognizable_hold_inside_corrupt_projection() {
        let hold = serde_json::to_string(&PackageState::CleanupIncomplete {
            message: "retained process ownership is uncertain".into(),
        })
        .unwrap();
        for damaged in [
            format!(r#"{{"packages":{{"demo@0.1.0":{{"state":{hold}}}}}}}"#),
            format!(r#"{{"packages":{{"demo@0.1.0":{{"state":{hold}}}}}"#),
            format!("invalid prefix {hold} invalid suffix"),
            format!(
                "invalid prefix {}",
                hold.replace("cleanup_incomplete", r"cleanup_\u0069ncomplete")
            ),
        ] {
            let td = tempdir().expect("tempdir");
            let events_path = events::events_path(td.path());
            write_events(
                &events_path,
                vec![event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".into(),
                        package_count: 1,
                    },
                )],
            );
            let state_path = execution_state::state_path(td.path());
            std::fs::write(&state_path, &damaged).unwrap();
            let events_before = std::fs::read(&events_path).unwrap();

            let error = rebuild_state_file_from_events(td.path(), options()).unwrap_err();

            assert!(format!("{error:#}").contains("cannot rebuild away retained cleanup hold"));
            assert_eq!(std::fs::read_to_string(state_path).unwrap(), damaged);
            assert_eq!(std::fs::read(events_path).unwrap(), events_before);
        }
    }

    #[test]
    fn rebuild_ignores_events_without_package_labels() {
        let td = tempdir().expect("tempdir");
        let events_path = td.path().join("events.jsonl");
        write_events(
            &events_path,
            vec![
                event(
                    0,
                    "all",
                    EventType::PlanCreated {
                        plan_id: "plan-123".to_string(),
                        package_count: 1,
                    },
                ),
                event(1, "all", EventType::ExecutionStarted),
                event(
                    2,
                    "",
                    EventType::PublishReconciled {
                        outcome: ReconciliationOutcome::Published {
                            attempts: 1,
                            elapsed_ms: 1,
                        },
                    },
                ),
                event(
                    3,
                    "demo@0.1.0",
                    EventType::PublishReconciling {
                        method: ReadinessMethod::Both,
                    },
                ),
            ],
        );

        let state = rebuild_state_from_events(&events_path, options()).expect("rebuild");

        assert!(state.packages.is_empty());
    }
}
