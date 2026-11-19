//! Canonical per-package execution and scheduler support.
//!
//! `publish_package` owns Cargo invocation, retries, readiness,
//! reconciliation, persistence, receipts, and notifications for one crate.
//! The parallel scheduler remains in `engine::parallel`; the module boundary
//! deliberately keeps package execution independent from scheduling.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
mod atomic_transitions_tests;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};

use crate::ops::cargo;
use crate::plan::PlannedWorkspace;
use crate::registry::RegistryClient;
use crate::runtime::execution::{
    backoff_delay, classify_cargo_failure, pkg_key, registry_aware_backoff, retry_after_delay,
    retry_next_attempt_at, short_state,
};
use crate::state::events;
use crate::state::execution_state as state;
use crate::state::upload_evidence::UploadWitness;
use shipper_types::{
    AttemptDetail, AttemptEvidence, ErrorClass, EventType, ExecutionState, PackageEvidence,
    PackageReceipt, PackageState, PlannedPackage, PublishEvent, PublishRegime, ReadinessConfig,
    ReadinessEvidence, ReadinessMethod, ReconciliationOutcome, RuntimeOptions,
};

use crate::engine::parallel::SendReporter;
use crate::engine::parallel::policy::policy_effects;
use crate::engine::parallel::webhook::{WebhookEvent, maybe_send_event};
use crate::engine::reconcile::reconcile_ambiguous_upload;

/// Result of publishing a single package (for parallel execution)
#[derive(Debug)]
pub(crate) struct PackagePublishResult {
    pub(crate) result: anyhow::Result<PackageReceipt>,
}

/// Execute packages in plan order through the canonical package executor.
///
/// This is the sequential scheduler counterpart to
/// `parallel::scheduler::run_publish_level`: it owns ordering and resume
/// gating, while `publish_package_with_timeout` owns package behavior.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_sequential_scheduler(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    st: &mut ExecutionState,
    state_dir: &Path,
    reg: &RegistryClient,
    event_log: &mut events::EventLog,
    events_path: &Path,
    reporter: &mut dyn crate::engine::Reporter,
) -> Result<Vec<PackageReceipt>> {
    if let Some(resume_from) = opts.resume_from.as_deref()
        && !ws.plan.packages.iter().any(|p| p.name == resume_from)
    {
        bail!("resume package not found in plan: {resume_from}");
    }

    let st_arc = Arc::new(Mutex::new(st.clone()));
    let event_log_arc = Arc::new(Mutex::new(std::mem::replace(
        event_log,
        events::EventLog::new(),
    )));
    let send_reporter = Arc::new(crate::engine::parallel::SendReporter::default());
    let mut reached_resume_point = opts.resume_from.is_none();
    let run_result = (|| -> Result<Vec<PackageReceipt>> {
        let mut receipts = Vec::new();
        for package in &ws.plan.packages {
            let key = pkg_key(&package.name, &package.version);
            let progress = st_arc
                .lock()
                .map_err(|_| {
                    anyhow::anyhow!("execution state lock poisoned before package execution")
                })?
                .packages
                .get(&key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing package progress in state"))?;

            if !reached_resume_point
                && matches!(
                    crate::engine::publish::resume::apply_resume_from_gate(
                        package,
                        &progress,
                        opts,
                        &mut reached_resume_point,
                        reporter,
                    ),
                    crate::engine::publish::resume::ResumeGate::Skip
                )
            {
                // resume_from scheduling gate: packages before the resume
                // point never enter the executor. Terminal packages still
                // emit a trusted resume-skip event so events.jsonl remains
                // legible; non-terminal packages are silent scheduler skips.
                if matches!(
                    progress.state,
                    PackageState::Published | PackageState::Skipped { .. }
                ) {
                    let mut log = event_log_arc.lock().map_err(|_| {
                        anyhow::anyhow!("event log lock poisoned before resume skip")
                    })?;
                    crate::engine::publish::resume::record_terminal_resume_skip_event(
                        &progress,
                        &format!("{}@{}", package.name, package.version),
                        events_path,
                        &mut log,
                    )?;
                }
                continue;
            }

            // Already-terminal packages (Published / Skipped) are handled
            // inside publish_package_with_timeout so sequential and parallel
            // share the same resume-skip event + receipt shape (#153).
            // Keep the host reporter on its calling thread while the same
            // canonical executor owns Cargo, retry sleeps, and transitions.
            // Only one package worker is live in sequential mode.
            let result = std::thread::scope(|scope| {
                let handle = scope.spawn(|| {
                    publish_package_with_timeout(
                        package,
                        ws,
                        opts,
                        reg,
                        &st_arc,
                        state_dir,
                        &event_log_arc,
                        events_path,
                        &send_reporter,
                        sequential_cargo_timeout(opts),
                    )
                });
                while !handle.is_finished() {
                    send_reporter.wait_for_messages();
                    crate::engine::parallel::replay_buffered_messages_to_host(
                        reporter,
                        &send_reporter,
                    );
                }
                let result = handle.join();
                crate::engine::parallel::replay_buffered_messages_to_host(reporter, &send_reporter);
                result
                    .map_err(|_| anyhow::anyhow!("sequential publish worker join failed for {key}"))
            })?;

            match result.result {
                Ok(receipt) => receipts.push(receipt),
                Err(error) => return Err(error),
            }
        }

        crate::engine::parallel::replay_buffered_messages_to_host(reporter, &send_reporter);
        Ok(receipts)
    })();

    match run_result {
        Ok(receipts) => {
            synchronize_sequential_state(st, event_log, &st_arc, &event_log_arc)?;
            Ok(receipts)
        }
        Err(error) => {
            if let Err(sync_error) =
                synchronize_sequential_state(st, event_log, &st_arc, &event_log_arc)
            {
                Err(error.context(format!(
                    "additionally failed to synchronize sequential state: {sync_error}"
                )))
            } else {
                Err(error)
            }
        }
    }
}

fn synchronize_sequential_state(
    state: &mut ExecutionState,
    event_log: &mut events::EventLog,
    state_arc: &Arc<Mutex<ExecutionState>>,
    event_log_arc: &Arc<Mutex<events::EventLog>>,
) -> Result<()> {
    *state = state_arc
        .lock()
        .map_err(|_| anyhow::anyhow!("execution state lock poisoned after package execution"))?
        .clone();
    *event_log = std::mem::replace(
        &mut *event_log_arc
            .lock()
            .map_err(|_| anyhow::anyhow!("event log lock poisoned after package execution"))?,
        events::EventLog::new(),
    );
    Ok(())
}

/// Build a poison failure for `publish_package`. The `PackagePublishResult`
/// return shape (not `Result`) means the `let-else` arms at each lock site
/// can't use `?`; this helper keeps them to one line and guarantees every
/// poison site reports the same message shape.
fn poisoned_lock(resource: &str) -> PackagePublishResult {
    PackagePublishResult {
        result: Err(anyhow::anyhow!(
            "{resource} lock poisoned during parallel publish"
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_transition(
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
    event: PublishEvent,
) -> Result<()> {
    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned during package transition"))?;
    let mut state = st
        .lock()
        .map_err(|_| anyhow::anyhow!("execution state lock poisoned during package transition"))?;
    crate::engine::transition::commit(
        &mut state,
        state_dir,
        &mut log,
        events_path,
        key,
        new_state,
        event,
    )
}

#[allow(clippy::too_many_arguments)]
fn commit_pending_transition(
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
    pending_events: Vec<PublishEvent>,
) -> Result<()> {
    with_package_event_batch(st, event_log, key, pending_events, |state, log| {
        crate::engine::transition::commit_pending(
            state,
            state_dir,
            log,
            events_path,
            key,
            new_state,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn commit_attempt_transition(
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    key: &str,
    attempt: u32,
    event: PublishEvent,
) -> Result<()> {
    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned during package attempt"))?;
    let mut state = st
        .lock()
        .map_err(|_| anyhow::anyhow!("execution state lock poisoned during package attempt"))?;
    crate::engine::transition::commit_attempt(
        &mut state,
        state_dir,
        &mut log,
        events_path,
        key,
        attempt,
        event,
    )
}

#[allow(clippy::too_many_arguments)]
fn commit_with_attempt_detail_transition(
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
    event: PublishEvent,
    detail: AttemptDetail,
) -> Result<()> {
    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned during package transition"))?;
    let mut state = st
        .lock()
        .map_err(|_| anyhow::anyhow!("execution state lock poisoned during package transition"))?;
    crate::engine::transition::commit_with_attempt_detail(
        &mut state,
        state_dir,
        &mut log,
        events_path,
        key,
        new_state,
        event,
        detail,
    )
}

#[allow(clippy::too_many_arguments)]
fn commit_pending_with_attempt_detail_transition(
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
    detail: AttemptDetail,
    pending_events: Vec<PublishEvent>,
) -> Result<()> {
    with_package_event_batch(st, event_log, key, pending_events, |state, log| {
        crate::engine::transition::commit_pending_with_attempt_detail(
            state,
            state_dir,
            log,
            events_path,
            key,
            new_state,
            detail,
        )
    })
}

fn validate_package_event_batch(key: &str, pending_events: &[PublishEvent]) -> Result<()> {
    if pending_events.is_empty() {
        bail!("missing domain event batch for package transition: {key}");
    }
    for event in pending_events {
        if event.package != key {
            bail!(
                "package transition key '{}' does not match batch event package '{}'",
                key,
                event.package
            );
        }
    }
    Ok(())
}

// Own explanatory events until their projection is ready. Appending the batch
// and committing it share one log guard, so another worker cannot replace or
// flush its tail between those operations. The callback only persists state;
// callers keep Cargo, registry queries, reporting and sleeps outside this scope.
fn with_package_event_batch(
    st: &Arc<Mutex<ExecutionState>>,
    event_log: &Arc<Mutex<events::EventLog>>,
    key: &str,
    pending_events: Vec<PublishEvent>,
    commit: impl FnOnce(&mut ExecutionState, &mut events::EventLog) -> Result<()>,
) -> Result<()> {
    validate_package_event_batch(key, &pending_events)?;
    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned during package transition"))?;
    for event in pending_events {
        log.record(event);
    }
    // Retain the batch in the shared buffer even if the state mutex is poisoned.
    let mut state = st
        .lock()
        .map_err(|_| anyhow::anyhow!("execution state lock poisoned during package transition"))?;
    commit(&mut state, &mut log)
}

/// Emit a [`EventType::RetryBackoffStarted`] event + a human-readable warn
/// line through the Reporter, then `thread::sleep(delay)`. Used at every
/// retry-backoff site in the publish loop so operators never stare at a
/// silent CI log during the wait window. See #91.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_retry_backoff(
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    reporter: &Arc<SendReporter>,
    pkg_label: &str,
    pkg_name: &str,
    pkg_version: &str,
    attempt: u32,
    max_attempts: u32,
    delay: std::time::Duration,
    next_attempt_at: DateTime<Utc>,
    reason: ErrorClass,
    message: &str,
) -> Result<()> {
    record_retry_backoff(
        event_log,
        pkg_label,
        attempt,
        max_attempts,
        delay,
        next_attempt_at,
        &reason,
        message,
    )?;
    flush_event_log(event_log, events_path)?;
    wait_after_retry(
        reporter,
        pkg_name,
        pkg_version,
        attempt,
        max_attempts,
        delay,
        reason,
        message,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_retry_backoff(
    event_log: &Arc<Mutex<events::EventLog>>,
    pkg_label: &str,
    attempt: u32,
    max_attempts: u32,
    delay: std::time::Duration,
    next_attempt_at: DateTime<Utc>,
    reason: &ErrorClass,
    message: &str,
) -> Result<()> {
    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned during retry scheduling"))?;
    for event in retry_backoff_events(
        pkg_label,
        attempt,
        max_attempts,
        delay,
        next_attempt_at,
        reason,
        message,
    ) {
        log.record(event);
    }
    Ok(())
}

fn retry_backoff_events(
    pkg_label: &str,
    attempt: u32,
    max_attempts: u32,
    delay: Duration,
    next_attempt_at: DateTime<Utc>,
    reason: &ErrorClass,
    message: &str,
) -> Vec<PublishEvent> {
    vec![
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::RetryScheduled {
                attempt,
                max_attempts,
                delay_ms: delay.as_millis() as u64,
                next_attempt_at,
                reason: reason.clone(),
                message: message.to_string(),
            },
            package: pkg_label.to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishWaiting {
                reason: "retry backoff".to_string(),
                delay_ms: delay.as_millis() as u64,
                until: next_attempt_at,
            },
            package: pkg_label.to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::RetryBackoffStarted {
                attempt,
                max_attempts,
                delay_ms: delay.as_millis() as u64,
                next_attempt_at,
                reason: reason.clone(),
                message: message.to_string(),
            },
            package: pkg_label.to_string(),
        },
    ]
}

fn rate_limit_observed_event(
    pkg_label: &str,
    is_new_crate: bool,
    retry_after: Option<std::time::Duration>,
    message: &str,
) -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::RateLimitObserved {
            is_new_crate,
            retry_after_ms: retry_after.map(|delay| delay.as_millis() as u64),
            message: message.to_string(),
        },
        package: pkg_label.to_string(),
    }
}

fn flush_event_log(event_log: &Arc<Mutex<events::EventLog>>, events_path: &Path) -> Result<()> {
    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned while flushing events"))?;
    log.write_to_file(events_path)?;
    log.clear();
    Ok(())
}

fn record_readiness_event(
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    event: PublishEvent,
) -> Result<()> {
    let mut log = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned while recording readiness event"))?;
    log.record(event);
    log.write_to_file(events_path)?;
    log.clear();
    Ok(())
}

fn record_readiness_started(
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    method: ReadinessMethod,
    pkg_label: &str,
) -> Result<()> {
    record_readiness_event(
        event_log,
        events_path,
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessStarted { method },
            package: pkg_label.to_string(),
        },
    )
}

fn record_readiness_error(
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    duration: Duration,
    pkg_label: &str,
) -> Result<()> {
    record_readiness_event(
        event_log,
        events_path,
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessError {
                duration_ms: duration.as_millis() as u64,
            },
            package: pkg_label.to_string(),
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn wait_after_retry(
    reporter: &Arc<SendReporter>,
    pkg_name: &str,
    pkg_version: &str,
    attempt: u32,
    max_attempts: u32,
    delay: std::time::Duration,
    reason: ErrorClass,
    message: &str,
) {
    reporter.retry_wait(
        pkg_name,
        pkg_version,
        attempt,
        max_attempts,
        delay,
        reason,
        message,
    );
}

fn write_reconciliation_report_best_effort(
    state_dir: &Path,
    ws: &PlannedWorkspace,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    reporter: &Arc<SendReporter>,
) -> bool {
    // Call after the package transition releases its guards. The complete
    // durable-event snapshot and report replacement share the append gate:
    // locking only the rename would still let an older snapshot win later.
    // Transitions acquire event log before execution state; this path never
    // acquires execution state or holds the gate during Cargo/registry work.
    let result = event_log
        .lock()
        .map_err(|_| anyhow::anyhow!("event log lock poisoned while writing reconciliation report"))
        .and_then(|_log_guard| {
            crate::state::reconciliation::write_report_from_events(
                state_dir,
                &ws.plan.plan_id,
                &ws.plan.registry,
                events_path,
            )
        });
    // Report errors after releasing the persistence gate. A warning/false
    // result remains failure evidence; it never authorizes a recovery.
    if let Err(err) = result {
        reporter.warn(&format!("failed to write reconciliation report: {err:#}"));
        return false;
    }
    true
}

/// Sequential publishing uses the same finite per-package ceiling as the
/// parallel executor. The option remains stored under `parallel` for one
/// compatibility cycle; the runtime policy applies in both scheduling modes.
pub(crate) fn sequential_cargo_timeout(opts: &RuntimeOptions) -> Option<Duration> {
    Some(opts.parallel.per_package_timeout)
}

/// Publish a single package with retries (parallel-safe version)
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_package(
    p: &PlannedPackage,
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    reporter: &Arc<SendReporter>,
) -> PackagePublishResult {
    publish_package_with_timeout(
        p,
        ws,
        opts,
        reg,
        st,
        state_dir,
        event_log,
        events_path,
        reporter,
        Some(opts.parallel.per_package_timeout),
    )
}

/// Execute a package with an explicit Cargo timeout policy.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_package_with_timeout(
    p: &PlannedPackage,
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    reg: &RegistryClient,
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    reporter: &Arc<SendReporter>,
    cargo_timeout: Option<Duration>,
) -> PackagePublishResult {
    let key = pkg_key(&p.name, &p.version);
    let pkg_label = format!("{}@{}", p.name, p.version);
    let started_at = Utc::now();
    let start_instant = Instant::now();

    // Shared terminal gate (#153): packages already Published/Skipped in
    // state are trusted as complete by both sequential and parallel. Emit a
    // resume-skip event without PackageStarted / cargo work so events.jsonl
    // remains auditable and finalization's trusted-resume-skip allowance
    // continues to apply (state stays terminal; rebuild projects Skipped).
    let progress = {
        let Ok(state) = st.lock() else {
            return poisoned_lock("execution state");
        };
        if let Err(error) = crate::engine::publish::admission::ensure_no_cleanup_hold(&state) {
            return PackagePublishResult { result: Err(error) };
        }
        match state.packages.get(&key) {
            Some(progress) => progress.clone(),
            None => {
                return PackagePublishResult {
                    result: Err(anyhow::anyhow!("missing package progress in state")),
                };
            }
        }
    };
    if let Err(error) = crate::ops::process::interruption::check() {
        return PackagePublishResult { result: Err(error) };
    }

    if matches!(
        progress.state,
        PackageState::Published | PackageState::Skipped { .. }
    ) {
        let short = short_state(&progress.state);
        reporter.info(&format!(
            "{}@{}: already complete ({})",
            p.name, p.version, short
        ));

        {
            let Ok(mut log) = event_log.lock() else {
                return poisoned_lock("event log");
            };
            if let Err(err) = crate::engine::publish::resume::record_terminal_resume_skip_event(
                &progress,
                &pkg_label,
                events_path,
                &mut log,
            ) {
                return PackagePublishResult {
                    result: Err(err.context(format!(
                        "{}@{}: failed to write package-skip event",
                        p.name, p.version
                    ))),
                };
            }
        }

        return PackagePublishResult {
            result: Ok(PackageReceipt {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: progress.attempts,
                state: progress.state.clone(),
                started_at,
                finished_at: Utc::now(),
                duration_ms: start_instant.elapsed().as_millis(),
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
                compromised_at: None,
                compromised_by: None,
                superseded_by: None,
            }),
        };
    }

    // Resolve legacy visibility-only work under the event writer lock, so a
    // sibling cannot append while we read the retained upload evidence.
    let verify_retained_upload = {
        let Ok(mut log) = event_log.lock() else {
            return poisoned_lock("event log");
        };
        let verify_legacy_exhaustion = if matches!(
            progress.state,
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                ..
            }
        ) {
            match events::EventLog::read_from_file(events_path) {
                Ok(retained) => retained
                    .all_events()
                    .iter()
                    .filter(|event| event.package == pkg_label)
                    .fold(UploadWitness::default(), |witness, event| {
                        witness.observe(&event.event_type)
                    })
                    .requires_legacy_visibility_check(&progress),
                Err(error) => return PackagePublishResult { result: Err(error) },
            }
        } else {
            false
        };
        log.record(PublishEvent {
            timestamp: started_at,
            event_type: EventType::PackageStarted {
                name: p.name.clone(),
                version: p.version.clone(),
            },
            package: pkg_label.clone(),
        });
        if let Err(err) = log.write_to_file(events_path) {
            return PackagePublishResult {
                result: Err(err.context(format!(
                    "{}@{}: failed to write package-start event",
                    p.name, p.version
                ))),
            };
        }
        log.clear();
        verify_legacy_exhaustion || progress.state == PackageState::Uploaded
    };

    let (mut attempt, ambiguous_prior, check_existing_version) = {
        // Registry visibility may have changed between invocations. A retained
        // retryable failure needs the same pre-attempt check as pending work;
        // keep Ambiguous and Uploaded on their dedicated reconciliation paths.
        let check_existing_version = matches!(
            &progress.state,
            PackageState::Pending
                | PackageState::Failed {
                    class: ErrorClass::Retryable,
                    ..
                }
        );
        let ambiguous_prior = match &progress.state {
            PackageState::Ambiguous { message } => Some(message.clone()),
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message,
            } if !verify_retained_upload => Some(message.clone()),
            _ => None,
        };
        (progress.attempts, ambiguous_prior, check_existing_version)
    };

    reporter.info(&format!("{}@{}: publishing...", p.name, p.version));
    let mut last_err: Option<(ErrorClass, String)> = None;
    let mut attempt_evidence: Vec<AttemptEvidence> = Vec::new();
    let mut readiness_evidence: Vec<ReadinessEvidence> = Vec::new();
    let mut cargo_succeeded = false;

    // Apply policy effects for readiness (Fix 7: parallel mode must respect PublishPolicy::Fast)
    let effects = policy_effects(opts);
    let readiness_config = ReadinessConfig {
        enabled: effects.readiness_enabled,
        ..opts.readiness.clone()
    };

    // Resume-path reconciliation (#99 follow-on): if a prior run left this
    // package in PackageState::Ambiguous, reconcile against registry truth
    // BEFORE entering the retry loop so we never re-upload a crate whose
    // prior upload may have actually succeeded.
    if let Some(prior_reason) = ambiguous_prior {
        reporter.warn(&format!(
            "{}@{}: resume found ambiguous state ({}); reconciling against registry",
            p.name, p.version, prior_reason
        ));
        let mut pending_events = vec![PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciling {
                method: readiness_config.method,
            },
            package: pkg_label.clone(),
        }];

        let (outcome, _evidence) =
            reconcile_ambiguous_upload(reg, &p.name, &p.version, &readiness_config);

        pending_events.push(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciled {
                outcome: outcome.clone(),
            },
            package: pkg_label.clone(),
        });
        let reconciliation_report_path = state::reconciliation_path(state_dir);

        match outcome {
            ReconciliationOutcome::Published { .. } => {
                pending_events.push(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackagePublished {
                        duration_ms: start_instant.elapsed().as_millis() as u64,
                    },
                    package: pkg_label.clone(),
                });
                if let Err(e) = commit_pending_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    PackageState::Published,
                    pending_events,
                ) {
                    return PackagePublishResult { result: Err(e) };
                }
                let _ = write_reconciliation_report_best_effort(
                    state_dir,
                    ws,
                    event_log,
                    events_path,
                    reporter,
                );
                reporter.info(&format!(
                    "{}@{}: reconciliation outcome: Published; action: mark published and continue without republish (evidence: {})",
                    p.name,
                    p.version,
                    reconciliation_report_path.display()
                ));
                maybe_send_event(
                    &opts.webhook,
                    WebhookEvent::PublishSucceeded {
                        plan_id: ws.plan.plan_id.clone(),
                        package_name: p.name.clone(),
                        package_version: p.version.clone(),
                        duration_ms: start_instant.elapsed().as_millis() as u64,
                    },
                );
                return PackagePublishResult {
                    result: Ok(PackageReceipt {
                        name: p.name.clone(),
                        version: p.version.clone(),
                        attempts: attempt,
                        state: PackageState::Published,
                        started_at,
                        finished_at: Utc::now(),
                        duration_ms: start_instant.elapsed().as_millis(),
                        evidence: PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                        compromised_at: None,
                        compromised_by: None,
                        superseded_by: None,
                    }),
                };
            }
            ReconciliationOutcome::NotPublished { .. } => {
                if let Err(e) = commit_pending_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    PackageState::Pending,
                    pending_events,
                ) {
                    return PackagePublishResult { result: Err(e) };
                }
                let _ = write_reconciliation_report_best_effort(
                    state_dir,
                    ws,
                    event_log,
                    events_path,
                    reporter,
                );
                reporter.info(&format!(
                    "{}@{}: reconciliation outcome: NotPublished; action: retry under publish policy (evidence: {})",
                    p.name,
                    p.version,
                    reconciliation_report_path.display()
                ));
                // Fall through to the normal retry loop below.
            }
            ReconciliationOutcome::StillUnknown { reason, .. } => {
                if let Err(e) = commit_pending_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    PackageState::Ambiguous {
                        message: reason.clone(),
                    },
                    pending_events,
                ) {
                    return PackagePublishResult { result: Err(e) };
                }
                let reconciliation_written = write_reconciliation_report_best_effort(
                    state_dir,
                    ws,
                    event_log,
                    events_path,
                    reporter,
                );
                reporter.error(&format!(
                    "{}@{}: reconciliation outcome: StillUnknown; action: stop before blind retry; operator action required (evidence: {}): {}",
                    p.name,
                    p.version,
                    reconciliation_report_path.display(),
                    reason
                ));
                maybe_send_event(
                    &opts.webhook,
                    WebhookEvent::PublishFailed {
                        plan_id: ws.plan.plan_id.clone(),
                        package_name: p.name.clone(),
                        package_version: p.version.clone(),
                        error_class: format!("{:?}", ErrorClass::Ambiguous),
                        message: format!("resume reconciliation still inconclusive: {reason}"),
                    },
                );
                return PackagePublishResult {
                    result: Err(crate::engine::PublishStillUnknownError {
                        message: format!(
                            "{}@{}: resume reconciliation still inconclusive; operator action required. Prior reason: {}",
                            p.name, p.version, reason
                        ),
                        reconciliation_written,
                    }
                    .into()),
                };
            }
        }
    }

    if verify_retained_upload {
        reporter.info(&format!(
            "{}@{}: resuming from uploaded (verifying registry visibility)",
            p.name, p.version
        ));
        let readiness_started_at = Instant::now();
        if let Err(e) =
            record_readiness_started(event_log, events_path, readiness_config.method, &pkg_label)
        {
            return PackagePublishResult { result: Err(e) };
        }
        let mut emit_readiness_event =
            |event| record_readiness_event(event_log, events_path, event);
        match reg.is_version_visible_with_backoff_and_events(
            &p.name,
            &p.version,
            &readiness_config,
            &mut emit_readiness_event,
        ) {
            Ok((visible, checks)) => {
                readiness_evidence = checks;
                if visible {
                    if let Err(e) = record_readiness_event(
                        event_log,
                        events_path,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::ReadinessComplete {
                                duration_ms: readiness_started_at.elapsed().as_millis() as u64,
                                attempts: readiness_evidence.last().map_or(0, |poll| poll.attempt),
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        let _ = record_readiness_error(
                            event_log,
                            events_path,
                            readiness_started_at.elapsed(),
                            &pkg_label,
                        );
                        return PackagePublishResult { result: Err(e) };
                    }
                    if let Err(e) = commit_transition(
                        st,
                        state_dir,
                        event_log,
                        events_path,
                        &key,
                        PackageState::Published,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PackagePublished {
                                duration_ms: start_instant.elapsed().as_millis() as u64,
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        return PackagePublishResult {
                            result: Err(e.context(format!(
                                "{}@{}: failed to promote uploaded package to published",
                                p.name, p.version
                            ))),
                        };
                    }
                    maybe_send_event(
                        &opts.webhook,
                        WebhookEvent::PublishSucceeded {
                            plan_id: ws.plan.plan_id.clone(),
                            package_name: p.name.clone(),
                            package_version: p.version.clone(),
                            duration_ms: start_instant.elapsed().as_millis() as u64,
                        },
                    );
                    return PackagePublishResult {
                        result: Ok(PackageReceipt {
                            name: p.name.clone(),
                            version: p.version.clone(),
                            attempts: attempt,
                            state: PackageState::Published,
                            started_at,
                            finished_at: Utc::now(),
                            duration_ms: start_instant.elapsed().as_millis(),
                            evidence: PackageEvidence {
                                attempts: vec![],
                                readiness_checks: readiness_evidence,
                            },
                            compromised_at: None,
                            compromised_by: None,
                            superseded_by: None,
                        }),
                    };
                }

                if let Err(e) = record_readiness_event(
                    event_log,
                    events_path,
                    PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::ReadinessTimeout {
                            max_wait_ms: readiness_config.max_total_wait.as_millis() as u64,
                        },
                        package: pkg_label.clone(),
                    },
                ) {
                    return PackagePublishResult { result: Err(e) };
                }

                // Cargo already accepted this upload. Registry absence
                // during readiness cannot authorize another submission.
                return PackagePublishResult {
                    result: Err(anyhow::anyhow!(
                        "{}@{}: previously accepted upload is not yet visible on the registry",
                        p.name,
                        p.version
                    )),
                };
            }
            Err(error) => {
                if let Err(close_error) = record_readiness_error(
                    event_log,
                    events_path,
                    readiness_started_at.elapsed(),
                    &pkg_label,
                ) {
                    return PackagePublishResult {
                        result: Err(close_error),
                    };
                }
                return PackagePublishResult {
                    result: Err(error.context(format!(
                        "{}@{}: failed to verify uploaded package visibility before publish",
                        p.name, p.version
                    ))),
                };
            }
        }
    }

    if check_existing_version {
        match reg.version_exists(&p.name, &p.version) {
            Ok(true) => {
                reporter.info(&format!(
                    "{}@{}: already published (skipping)",
                    p.name, p.version
                ));

                let skipped = PackageState::Skipped {
                    reason: "already published".to_string(),
                };
                if let Err(err) = commit_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    skipped.clone(),
                    PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PackageSkipped {
                            reason: "already published".to_string(),
                        },
                        package: pkg_label.clone(),
                    },
                ) {
                    return PackagePublishResult {
                        result: Err(err.context(format!(
                            "{}@{}: failed to mark package as already published",
                            p.name, p.version
                        ))),
                    };
                }

                return PackagePublishResult {
                    result: Ok(PackageReceipt {
                        name: p.name.clone(),
                        version: p.version.clone(),
                        attempts: attempt,
                        state: PackageState::Skipped {
                            reason: "already published".to_string(),
                        },
                        started_at,
                        finished_at: Utc::now(),
                        duration_ms: start_instant.elapsed().as_millis(),
                        evidence: PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                        compromised_at: None,
                        compromised_by: None,
                        superseded_by: None,
                    }),
                };
            }
            Ok(false) => {}
            Err(error) => {
                reporter.warn(&format!(
                    "{}@{}: existing-version check failed before publish; continuing publish: {error}",
                    p.name, p.version
                ));
            }
        }
    }

    // Registry-aware backoff (#94 / #106 PR 1): prefer the `PublishRegime`
    // that preflight stamped onto the `PlannedPackage`. That answer is
    // authoritative; when it is present we never re-query the registry
    // mid-retry for "is this a brand-new crate?"
    //
    // `None` here means an older plan / state.json predating the regime
    // field, or a test harness that constructed a `PlannedPackage`
    // directly without populating it. In that case we fall back to the
    // legacy lazy-cached behavior so we remain backward compatible.
    let mut is_new_crate_cached: Option<bool> = p.regime.map(PublishRegime::is_new_crate);

    while attempt < opts.max_attempts {
        if let Err(error) = crate::ops::process::interruption::check() {
            return PackagePublishResult { result: Err(error) };
        }
        attempt += 1;

        let command = format!(
            "cargo publish -p {} --registry {}",
            p.name, ws.plan.registry.name
        );

        reporter.info(&format!(
            "{}@{}: attempt {}/{}",
            p.name, p.version, attempt, opts.max_attempts
        ));

        if !cargo_succeeded {
            let attempt_started_at = Utc::now();
            if let Err(e) = commit_attempt_transition(
                st,
                state_dir,
                event_log,
                events_path,
                &key,
                attempt,
                PublishEvent {
                    timestamp: attempt_started_at,
                    event_type: EventType::PackageAttempted {
                        attempt,
                        command: command.clone(),
                        max_attempts: opts.max_attempts,
                    },
                    package: pkg_label.clone(),
                },
            ) {
                return PackagePublishResult { result: Err(e) };
            }

            let captured = match cargo::cargo_publish_with_capture(
                &ws.workspace_root,
                &p.name,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.no_verify,
                opts.output_lines,
                cargo_timeout,
            ) {
                Ok(o) => o,
                Err(mut e) => {
                    if let Some(no_launch) =
                        e.downcast_mut::<crate::ops::process::CommandNotLaunched>()
                    {
                        let _lease = no_launch.take_lease();
                        let message = "Cargo publish was not launched; check the executable and process setup before resuming".to_string();
                        let ended_at = Utc::now();
                        if let Err(persistence_error) = commit_with_attempt_detail_transition(
                            st,
                            state_dir,
                            event_log,
                            events_path,
                            &key,
                            PackageState::Failed {
                                class: ErrorClass::Retryable,
                                message: message.clone(),
                            },
                            PublishEvent {
                                timestamp: ended_at,
                                event_type: EventType::PackageNotLaunched {
                                    attempt,
                                    message: message.clone(),
                                },
                                package: pkg_label.clone(),
                            },
                            AttemptDetail {
                                package: p.name.clone(),
                                version: p.version.clone(),
                                attempt,
                                max_attempts: opts.max_attempts,
                                started_at: attempt_started_at,
                                ended_at,
                                error_class: Some(ErrorClass::Retryable),
                                next_attempt_at: None,
                                redacted_message: Some(message),
                            },
                        ) {
                            return PackagePublishResult {
                                result: Err(persistence_error.context(format!(
                                    "failed to record that Cargo did not launch: {e:#}"
                                ))),
                            };
                        }
                    }
                    reporter.error(&format!(
                        "{}@{}: cargo publish failed to execute: {}",
                        p.name, p.version, e
                    ));
                    return PackagePublishResult { result: Err(e) };
                }
            };
            if let Some(failure) = captured.cleanup_failure {
                let message = crate::engine::cleanup::message(failure);
                reporter.error(&format!("{}@{}: {message}", p.name, p.version));
                let detail = AttemptDetail {
                    package: p.name.clone(),
                    version: p.version.clone(),
                    attempt,
                    max_attempts: opts.max_attempts,
                    started_at: attempt_started_at,
                    ended_at: Utc::now(),
                    error_class: Some(ErrorClass::Ambiguous),
                    next_attempt_at: None,
                    redacted_message: Some(message.clone()),
                };
                return PackagePublishResult {
                    result: Err(crate::engine::cleanup::stop_publish(
                        st,
                        state_dir,
                        event_log,
                        events_path,
                        &key,
                        message,
                        detail,
                        &captured.output,
                    )),
                };
            }
            let capture_complete = captured.capture.is_complete();
            let out = captured.output;
            if !capture_complete {
                reporter.warn(&format!(
                    "{}@{}: Cargo diagnostic capture was incomplete; diagnostic tails omitted",
                    p.name, p.version
                ));
            }
            let attempt_ended_at = Utc::now();

            // Collect attempt evidence
            attempt_evidence.push(AttemptEvidence {
                attempt_number: attempt,
                command: command.clone(),
                exit_code: out.exit_code,
                stdout_tail: out.stdout_tail.clone(),
                stderr_tail: out.stderr_tail.clone(),
                timestamp: attempt_ended_at,
                duration: out.duration,
            });

            // Event: PackageOutput
            {
                let Ok(mut log) = event_log.lock() else {
                    return poisoned_lock("event log");
                };
                log.record(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageOutput {
                        stdout_tail: out.stdout_tail.clone(),
                        stderr_tail: out.stderr_tail.clone(),
                    },
                    package: pkg_label.clone(),
                });
                if let Err(err) = log.write_to_file(events_path) {
                    return PackagePublishResult {
                        result: Err(err.context(format!(
                            "{}@{}: failed to write package-output event",
                            p.name, p.version
                        ))),
                    };
                }
                log.clear();
            }

            if captured.interrupted || crate::ops::process::interruption::requested() {
                let message = "cargo publish interrupted; process cleanup confirmed; registry reconciliation required".to_string();
                let ended_at = Utc::now();
                let failure = PublishEvent {
                    timestamp: ended_at,
                    event_type: EventType::PackageFailed {
                        class: ErrorClass::Ambiguous,
                        message: message.clone(),
                    },
                    package: pkg_label.clone(),
                };
                let result = commit_pending_with_attempt_detail_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    PackageState::Failed {
                        class: ErrorClass::Ambiguous,
                        message: message.clone(),
                    },
                    AttemptDetail {
                        package: p.name.clone(),
                        version: p.version.clone(),
                        attempt,
                        max_attempts: opts.max_attempts,
                        started_at: attempt_started_at,
                        ended_at,
                        error_class: Some(ErrorClass::Ambiguous),
                        next_attempt_at: None,
                        redacted_message: Some(message),
                    },
                    // The first failure records the attempt outcome; the
                    // terminal failure seals it, as in ordinary exhaustion.
                    // A later run must not finalize these old attempts in its
                    // new parallel reconciliation order.
                    vec![failure.clone(), failure],
                );
                // `captured` retains the interruption lease until this event-
                // first transition finishes, including its failure path.
                return PackagePublishResult {
                    result: Err(match result {
                        Ok(()) => crate::ops::process::interruption::Interrupted.into(),
                        Err(error) => error.context(crate::ops::process::interruption::Interrupted),
                    }),
                };
            }
            if out.exit_code == 0 && !out.timed_out {
                let completion_timestamp = Utc::now();
                let attempt_detail = AttemptDetail {
                    package: p.name.clone(),
                    version: p.version.clone(),
                    attempt,
                    max_attempts: opts.max_attempts,
                    started_at: attempt_started_at,
                    ended_at: completion_timestamp,
                    error_class: None,
                    next_attempt_at: None,
                    redacted_message: None,
                };
                cargo_succeeded = true;
                // PackageUploaded is the durable checkpoint that proves cargo
                // accepted the upload and projects Uploaded on rebuild.
                if let Err(e) = commit_with_attempt_detail_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    PackageState::Uploaded,
                    PublishEvent {
                        timestamp: completion_timestamp,
                        event_type: EventType::PackageUploaded,
                        package: pkg_label.clone(),
                    },
                    attempt_detail,
                ) {
                    return PackagePublishResult { result: Err(e) };
                }
                drop(captured._interruption_lease);
                if let Err(error) = crate::ops::process::interruption::check() {
                    return PackagePublishResult { result: Err(error) };
                }
            } else {
                let failure_output = format!("{}\n{}", out.stderr_tail, out.stdout_tail);
                // Killing Cargo cannot establish whether the upload reached the
                // registry. The typed timeout owns this decision even when the
                // retained output is empty or matches a different error class.
                let (class, msg) = if out.timed_out {
                    (
                        ErrorClass::Ambiguous,
                        "cargo publish process timed out; registry reconciliation required"
                            .to_string(),
                    )
                } else if !capture_complete {
                    (
                        ErrorClass::Ambiguous,
                        "cargo publish diagnostic capture was incomplete; registry reconciliation required"
                            .to_string(),
                    )
                } else {
                    classify_cargo_failure(&out.stderr_tail, &out.stdout_tail)
                };
                let completion_timestamp = Utc::now();
                last_err = Some((class.clone(), msg.clone()));
                let mut attempt_detail = AttemptDetail {
                    package: p.name.clone(),
                    version: p.version.clone(),
                    attempt,
                    max_attempts: opts.max_attempts,
                    started_at: attempt_started_at,
                    ended_at: completion_timestamp,
                    error_class: Some(class.clone()),
                    next_attempt_at: None,
                    redacted_message: Some(msg.clone()),
                };

                // Preserve the unresolved upload under the process lease.
                // Registry I/O below can take longer than a controlled stop
                // allows, so release the lease as soon as replay is safe.
                if let Err(error) = commit_pending_with_attempt_detail_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    PackageState::Ambiguous {
                        message: msg.clone(),
                    },
                    attempt_detail.clone(),
                    vec![PublishEvent {
                        timestamp: completion_timestamp,
                        event_type: EventType::PackageUploadUnconfirmed {
                            class: class.clone(),
                            message: msg.clone(),
                        },
                        package: pkg_label.clone(),
                    }],
                ) {
                    return PackagePublishResult { result: Err(error) };
                }
                drop(captured._interruption_lease);
                if let Err(error) = crate::ops::process::interruption::check() {
                    return PackagePublishResult { result: Err(error) };
                }

                // Event: PackageFailed
                let mut pending_events = vec![PublishEvent {
                    timestamp: completion_timestamp,
                    event_type: EventType::PackageFailed {
                        class: class.clone(),
                        message: msg.clone(),
                    },
                    package: pkg_label.clone(),
                }];

                // On Ambiguous: never blind-retry. Reconcile against registry
                // truth first so we don't risk a duplicate upload. See #99.
                if class == ErrorClass::Ambiguous {
                    pending_events.push(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PublishReconciling {
                            method: readiness_config.method,
                        },
                        package: pkg_label.clone(),
                    });
                    reporter.warn(&format!(
                        "{}@{}: cargo exit ambiguous; reconciling against registry",
                        p.name, p.version
                    ));

                    let (outcome, reconcile_evidence) =
                        reconcile_ambiguous_upload(reg, &p.name, &p.version, &readiness_config);

                    pending_events.push(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PublishReconciled {
                            outcome: outcome.clone(),
                        },
                        package: pkg_label.clone(),
                    });
                    let reconciliation_report_path = state::reconciliation_path(state_dir);

                    match outcome {
                        ReconciliationOutcome::Published { .. } => {
                            reporter.info(&format!(
                                "{}@{}: reconciliation outcome: Published; registry shows version present; action: mark published and continue without retry (evidence: {})",
                                p.name,
                                p.version,
                                reconciliation_report_path.display()
                            ));
                            pending_events.push(PublishEvent {
                                timestamp: Utc::now(),
                                event_type: EventType::PackagePublished {
                                    duration_ms: start_instant.elapsed().as_millis() as u64,
                                },
                                package: pkg_label.clone(),
                            });
                            if let Err(e) = commit_pending_with_attempt_detail_transition(
                                st,
                                state_dir,
                                event_log,
                                events_path,
                                &key,
                                PackageState::Published,
                                attempt_detail.clone(),
                                pending_events,
                            ) {
                                return PackagePublishResult { result: Err(e) };
                            }
                            let _ = write_reconciliation_report_best_effort(
                                state_dir,
                                ws,
                                event_log,
                                events_path,
                                reporter,
                            );

                            // Preserve reconciliation evidence in the receipt.
                            // Do NOT emit PublishSucceeded webhook here; the
                            // end-of-function success path (below) handles that.
                            readiness_evidence = reconcile_evidence;
                            last_err = None;
                            break;
                        }
                        ReconciliationOutcome::NotPublished { .. } => {
                            if let Err(e) = commit_pending_transition(
                                st,
                                state_dir,
                                event_log,
                                events_path,
                                &key,
                                PackageState::Pending,
                                std::mem::take(&mut pending_events),
                            ) {
                                return PackagePublishResult { result: Err(e) };
                            }
                            let _ = write_reconciliation_report_best_effort(
                                state_dir,
                                ws,
                                event_log,
                                events_path,
                                reporter,
                            );
                            reporter.info(&format!(
                                "{}@{}: reconciliation outcome: NotPublished; registry still absent; action: retry under publish policy (evidence: {})",
                                p.name,
                                p.version,
                                reconciliation_report_path.display()
                            ));
                            // Safe to enter the normal Retryable path below;
                            // registry confirms no duplicate-upload risk.
                            // Preserve negative-polling evidence for the receipt.
                            readiness_evidence = reconcile_evidence;
                            if attempt >= opts.max_attempts {
                                let retryable = PackageState::Failed {
                                    class: ErrorClass::Retryable,
                                    message: msg.clone(),
                                };
                                pending_events.push(PublishEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::PackageFailed {
                                        class: ErrorClass::Retryable,
                                        message: msg.clone(),
                                    },
                                    package: pkg_label.clone(),
                                });
                                if let Err(e) = commit_pending_with_attempt_detail_transition(
                                    st,
                                    state_dir,
                                    event_log,
                                    events_path,
                                    &key,
                                    retryable,
                                    attempt_detail,
                                    pending_events,
                                ) {
                                    return PackagePublishResult { result: Err(e) };
                                }
                                return PackagePublishResult {
                                    result: Err(crate::engine::PublishRecoverableStopError {
                                        message: format!(
                                            "{}@{}: retry budget exhausted after registry confirmed NotPublished",
                                            p.name, p.version
                                        ),
                                        package: pkg_label.clone(),
                                        evidence_consistent: false,
                                    }
                                    .into()),
                                };
                            }
                        }
                        ReconciliationOutcome::StillUnknown { reason, .. } => {
                            let ambiguous_state = PackageState::Ambiguous {
                                message: reason.clone(),
                            };
                            if let Err(e) = commit_pending_with_attempt_detail_transition(
                                st,
                                state_dir,
                                event_log,
                                events_path,
                                &key,
                                ambiguous_state,
                                attempt_detail.clone(),
                                pending_events,
                            ) {
                                return PackagePublishResult { result: Err(e) };
                            }
                            let reconciliation_written = write_reconciliation_report_best_effort(
                                state_dir,
                                ws,
                                event_log,
                                events_path,
                                reporter,
                            );
                            reporter.error(&format!(
                                "{}@{}: reconciliation outcome: StillUnknown; action: stop before blind retry; operator action required (evidence: {}): {}",
                                p.name,
                                p.version,
                                reconciliation_report_path.display(),
                                reason
                            ));

                            // Notify operators: reconciliation was inconclusive
                            // and human judgment is required.
                            maybe_send_event(
                                &opts.webhook,
                                WebhookEvent::PublishFailed {
                                    plan_id: ws.plan.plan_id.clone(),
                                    package_name: p.name.clone(),
                                    package_version: p.version.clone(),
                                    error_class: format!("{:?}", ErrorClass::Ambiguous),
                                    message: format!("reconciliation inconclusive: {reason}"),
                                },
                            );

                            return PackagePublishResult {
                                result: Err(crate::engine::PublishStillUnknownError {
                                    message: format!(
                                        "{}@{}: reconciliation inconclusive: {}",
                                        p.name, p.version, reason
                                    ),
                                    reconciliation_written,
                                }
                                .into()),
                            };
                        }
                    }
                } else {
                    // Non-ambiguous failures keep the historical quick registry
                    // check. Ambiguous failures use the reconciliation state
                    // machine above so they are queried exactly once before a
                    // retry decision.
                    reporter.warn(&format!(
                        "{}@{}: cargo publish failed (exit={}); checking registry...",
                        p.name, p.version, out.exit_code
                    ));

                    match reg.version_exists(&p.name, &p.version) {
                        Ok(true) => {
                            reporter.info(&format!(
                                "{}@{}: version is present on registry; treating as published",
                                p.name, p.version
                            ));

                            pending_events.push(PublishEvent {
                                timestamp: Utc::now(),
                                event_type: EventType::PackagePublished {
                                    duration_ms: start_instant.elapsed().as_millis() as u64,
                                },
                                package: pkg_label.clone(),
                            });
                            if let Err(e) = commit_pending_with_attempt_detail_transition(
                                st,
                                state_dir,
                                event_log,
                                events_path,
                                &key,
                                PackageState::Published,
                                attempt_detail,
                                pending_events,
                            ) {
                                return PackagePublishResult { result: Err(e) };
                            }
                            last_err = None;
                            break;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            let error = anyhow::anyhow!(
                                "{}@{}: failed to verify registry visibility after cargo failure: {error}",
                                p.name,
                                p.version
                            );
                            // The unconfirmed-upload checkpoint is already durable.
                            // Unavailable evidence cannot turn it into a retryable
                            // failure: resume must reconcile before another Cargo run.
                            return PackagePublishResult { result: Err(error) };
                        }
                    }
                }

                match class {
                    ErrorClass::Permanent => {
                        let failed = PackageState::Failed {
                            class: class.clone(),
                            message: msg.clone(),
                        };
                        if let Err(e) = commit_pending_with_attempt_detail_transition(
                            st,
                            state_dir,
                            event_log,
                            events_path,
                            &key,
                            failed,
                            attempt_detail,
                            pending_events,
                        ) {
                            return PackagePublishResult { result: Err(e) };
                        }

                        // Send webhook notification: package failed
                        maybe_send_event(
                            &opts.webhook,
                            WebhookEvent::PublishFailed {
                                plan_id: ws.plan.plan_id.clone(),
                                package_name: p.name.clone(),
                                package_version: p.version.clone(),
                                error_class: format!("{:?}", class),
                                message: msg.clone(),
                            },
                        );

                        return PackagePublishResult {
                            result: Err(anyhow::anyhow!(
                                "{}@{}: permanent failure: {}",
                                p.name,
                                p.version,
                                msg
                            )),
                        };
                    }
                    ErrorClass::Retryable | ErrorClass::Ambiguous => {
                        // Ambiguous can only reach here if reconciliation
                        // returned NotPublished; registry confirms no
                        // duplicate-upload risk, so cargo retry is safe.
                        // Only query crate_exists when the error looks like
                        // a rate limit (saves a registry round-trip for
                        // generic network/transient failures).
                        let is_new_crate =
                            if crate::runtime::execution::looks_like_rate_limit(&failure_output) {
                                *is_new_crate_cached.get_or_insert_with(|| {
                                    !reg.crate_exists(&p.name).unwrap_or(true)
                                })
                            } else {
                                false
                            };
                        if attempt < opts.max_attempts {
                            if crate::runtime::execution::looks_like_rate_limit(&failure_output) {
                                pending_events.push(rate_limit_observed_event(
                                    &pkg_label,
                                    is_new_crate,
                                    retry_after_delay(&failure_output),
                                    &msg,
                                ));
                            }
                            let delay = registry_aware_backoff(
                                opts.base_delay,
                                opts.max_delay,
                                attempt,
                                opts.retry_strategy,
                                opts.retry_jitter,
                                is_new_crate,
                                &failure_output,
                            );
                            let next_attempt_at = retry_next_attempt_at(delay);
                            attempt_detail.next_attempt_at = Some(next_attempt_at);
                            pending_events.extend(retry_backoff_events(
                                &pkg_label,
                                attempt,
                                opts.max_attempts,
                                delay,
                                next_attempt_at,
                                &class,
                                &msg,
                            ));
                            if let Err(e) = commit_pending_with_attempt_detail_transition(
                                st,
                                state_dir,
                                event_log,
                                events_path,
                                &key,
                                PackageState::Pending,
                                attempt_detail,
                                pending_events,
                            ) {
                                return PackagePublishResult { result: Err(e) };
                            }
                            wait_after_retry(
                                reporter,
                                &p.name,
                                &p.version,
                                attempt,
                                opts.max_attempts,
                                delay,
                                class.clone(),
                                &msg,
                            );
                        }
                        // Exhaustion leaves the durable unconfirmed checkpoint
                        // in place until the final visibility check returns.
                        // A separate PackageFailed here would classify replay
                        // before the matching live state can be committed.
                    }
                }
                continue;
            }
        }

        // Readiness verification (runs after first cargo success + all retries)
        reporter.info(&format!(
            "{}@{}: cargo publish exited successfully; verifying...",
            p.name, p.version
        ));

        let readiness_started_at = Instant::now();
        if let Err(e) =
            record_readiness_started(event_log, events_path, readiness_config.method, &pkg_label)
        {
            return PackagePublishResult { result: Err(e) };
        }
        let mut emit_readiness_event =
            |event| record_readiness_event(event_log, events_path, event);
        let verify_result = reg.is_version_visible_with_backoff_and_events(
            &p.name,
            &p.version,
            &readiness_config,
            &mut emit_readiness_event,
        );

        match verify_result {
            Ok((visible, checks)) => {
                readiness_evidence = checks;
                if visible {
                    if let Err(e) = record_readiness_event(
                        event_log,
                        events_path,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::ReadinessComplete {
                                duration_ms: readiness_started_at.elapsed().as_millis() as u64,
                                attempts: readiness_evidence.last().map_or(0, |poll| poll.attempt),
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        let _ = record_readiness_error(
                            event_log,
                            events_path,
                            readiness_started_at.elapsed(),
                            &pkg_label,
                        );
                        return PackagePublishResult { result: Err(e) };
                    }
                    if let Err(e) = commit_transition(
                        st,
                        state_dir,
                        event_log,
                        events_path,
                        &key,
                        PackageState::Published,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PackagePublished {
                                duration_ms: start_instant.elapsed().as_millis() as u64,
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        return PackagePublishResult { result: Err(e) };
                    }
                    last_err = None;

                    break;
                } else {
                    if let Err(e) = record_readiness_event(
                        event_log,
                        events_path,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::ReadinessTimeout {
                                max_wait_ms: readiness_config.max_total_wait.as_millis() as u64,
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        return PackagePublishResult { result: Err(e) };
                    }
                    let message =
                        "published locally, but version not observed on registry within timeout";
                    last_err = Some((ErrorClass::Ambiguous, message.to_string()));
                    if attempt < opts.max_attempts {
                        let delay = backoff_delay(
                            opts.base_delay,
                            opts.max_delay,
                            attempt,
                            opts.retry_strategy,
                            opts.retry_jitter,
                        );
                        let next_attempt_at = retry_next_attempt_at(delay);
                        if let Err(err) = emit_retry_backoff(
                            event_log,
                            events_path,
                            reporter,
                            &pkg_label,
                            &p.name,
                            &p.version,
                            attempt,
                            opts.max_attempts,
                            delay,
                            next_attempt_at,
                            ErrorClass::Ambiguous,
                            message,
                        ) {
                            return PackagePublishResult {
                                result: Err(err.context(format!(
                                    "{}@{}: failed to schedule retry backoff",
                                    p.name, p.version
                                ))),
                            };
                        }
                    }
                }
            }
            Err(error) => {
                if let Err(close_error) = record_readiness_error(
                    event_log,
                    events_path,
                    readiness_started_at.elapsed(),
                    &pkg_label,
                ) {
                    return PackagePublishResult {
                        result: Err(close_error),
                    };
                }
                return PackagePublishResult { result: Err(error) };
            }
        }
    }

    // If package is still Uploaded (loop didn't run or readiness never checked), force a final check
    let is_still_uploaded = if last_err.is_none() {
        let Ok(state) = st.lock() else {
            return poisoned_lock("execution state");
        };
        matches!(
            state.packages.get(&key).map(|p| &p.state),
            Some(PackageState::Uploaded)
        )
    } else {
        false
    };

    if is_still_uploaded {
        match reg.version_exists(&p.name, &p.version) {
            Ok(true) => {
                if let Err(e) = commit_transition(
                    st,
                    state_dir,
                    event_log,
                    events_path,
                    &key,
                    PackageState::Published,
                    PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PackagePublished {
                            duration_ms: start_instant.elapsed().as_millis() as u64,
                        },
                        package: pkg_label.clone(),
                    },
                ) {
                    return PackagePublishResult { result: Err(e) };
                }
            }
            Ok(false) => {
                last_err = Some((
                    ErrorClass::Ambiguous,
                    "package was uploaded but not confirmed visible on registry".into(),
                ));
            }
            Err(error) => {
                return PackagePublishResult {
                    result: Err(anyhow::anyhow!(
                        "{}@{}: failed to verify final visibility for uploaded package: {error}",
                        p.name,
                        p.version
                    )),
                };
            }
        }
    }

    let finished_at = Utc::now();
    let duration_ms = start_instant.elapsed().as_millis();

    if let Some((class, msg)) = last_err {
        // Final chance: maybe it eventually showed up.
        let readiness_started_at = Instant::now();
        if let Err(e) =
            record_readiness_started(event_log, events_path, readiness_config.method, &pkg_label)
        {
            return PackagePublishResult { result: Err(e) };
        }
        let mut emit_readiness_event =
            |event| record_readiness_event(event_log, events_path, event);
        match reg.is_version_visible_with_backoff_and_events(
            &p.name,
            &p.version,
            &readiness_config,
            &mut emit_readiness_event,
        ) {
            Ok((visible, checks)) => {
                let check_count = checks.last().map_or(0, |poll| poll.attempt);
                if !checks.is_empty() {
                    readiness_evidence.extend(checks);
                }

                if visible {
                    if let Err(e) = record_readiness_event(
                        event_log,
                        events_path,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::ReadinessComplete {
                                duration_ms: readiness_started_at.elapsed().as_millis() as u64,
                                attempts: check_count,
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        let _ = record_readiness_error(
                            event_log,
                            events_path,
                            readiness_started_at.elapsed(),
                            &pkg_label,
                        );
                        return PackagePublishResult { result: Err(e) };
                    }
                    if let Err(e) = commit_transition(
                        st,
                        state_dir,
                        event_log,
                        events_path,
                        &key,
                        PackageState::Published,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PackagePublished {
                                duration_ms: duration_ms as u64,
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        return PackagePublishResult { result: Err(e) };
                    }
                } else {
                    if let Err(e) = record_readiness_event(
                        event_log,
                        events_path,
                        PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::ReadinessTimeout {
                                max_wait_ms: readiness_config.max_total_wait.as_millis() as u64,
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        return PackagePublishResult { result: Err(e) };
                    }
                    let error_class_str = format!("{:?}", class);
                    // Keep the accepted upload checkpoint when readiness
                    // fails. Subsequent runs can still verify visibility.
                    if !cargo_succeeded
                        && let Err(e) = commit_transition(
                            st,
                            state_dir,
                            event_log,
                            events_path,
                            &key,
                            PackageState::Failed {
                                class: class.clone(),
                                message: msg.clone(),
                            },
                            PublishEvent {
                                timestamp: Utc::now(),
                                event_type: EventType::PackageFailed {
                                    class: class.clone(),
                                    message: msg.clone(),
                                },
                                package: pkg_label.clone(),
                            },
                        )
                    {
                        return PackagePublishResult { result: Err(e) };
                    }

                    // Send webhook notification: publication did not finish
                    maybe_send_event(
                        &opts.webhook,
                        WebhookEvent::PublishFailed {
                            plan_id: ws.plan.plan_id.clone(),
                            package_name: p.name.clone(),
                            package_version: p.version.clone(),
                            error_class: error_class_str,
                            message: msg.clone(),
                        },
                    );

                    return PackagePublishResult {
                        result: Err(anyhow::anyhow!("{}@{}: failed: {}", p.name, p.version, msg)),
                    };
                }
            }
            Err(error) => {
                if let Err(close_error) = record_readiness_error(
                    event_log,
                    events_path,
                    readiness_started_at.elapsed(),
                    &pkg_label,
                ) {
                    return PackagePublishResult {
                        result: Err(close_error),
                    };
                }
                return PackagePublishResult {
                    result: Err(error.context(format!(
                        "{}@{}: failed to verify final visibility after last error",
                        p.name, p.version
                    ))),
                };
            }
        }
    }

    let (attempts, final_state) = {
        let Ok(state) = st.lock() else {
            return poisoned_lock("execution state");
        };
        let Some(progress) = state.packages.get(&key) else {
            return PackagePublishResult {
                result: Err(anyhow::anyhow!("missing package progress after execution")),
            };
        };
        (progress.attempts, progress.state.clone())
    };
    let successful = matches!(
        final_state,
        PackageState::Published | PackageState::Skipped { .. }
    );
    let pending_due_to_attempt_limit =
        matches!(final_state, PackageState::Pending) && attempts >= opts.max_attempts;
    if !successful && !pending_due_to_attempt_limit {
        return PackagePublishResult {
            result: Err(anyhow::anyhow!(
                "{}@{}: publish terminated without terminal state {:?}",
                p.name,
                p.version,
                final_state
            )),
        };
    }

    if successful {
        maybe_send_event(
            &opts.webhook,
            WebhookEvent::PublishSucceeded {
                plan_id: ws.plan.plan_id.clone(),
                package_name: p.name.clone(),
                package_version: p.version.clone(),
                duration_ms: duration_ms as u64,
            },
        );
    }

    PackagePublishResult {
        result: Ok(PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts,
            state: final_state,
            started_at,
            finished_at,
            duration_ms,
            evidence: PackageEvidence {
                attempts: attempt_evidence,
                readiness_checks: readiness_evidence,
            },
            compromised_at: None,
            compromised_by: None,
            superseded_by: None,
        }),
    }
}
