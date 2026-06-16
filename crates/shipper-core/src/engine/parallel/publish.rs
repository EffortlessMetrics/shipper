//! Single-package and single-level publish primitives for parallel execution.
//!
//! `publish_package` handles one crate with retries/backoff/readiness; it is
//! parallel-safe (all shared state goes through `Arc<Mutex<_>>`).
//! `run_publish_level` fans out a level's packages into concurrent threads,
//! batched by `parallel.max_concurrent`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};

use crate::ops::cargo;
use crate::plan::PlannedWorkspace;
use crate::runtime::execution::{
    append_attempt_detail, backoff_delay, classify_cargo_failure, pkg_key, registry_aware_backoff,
    retry_after_delay, retry_next_attempt_at, update_state_locked,
};
use crate::state::events;
use crate::state::execution_state as state;
use shipper_registry::HttpRegistryClient as RegistryClient;
use shipper_types::{
    AttemptDetail, AttemptEvidence, ErrorClass, EventType, ExecutionState, PackageEvidence,
    PackageReceipt, PackageState, PlannedPackage, PublishEvent, PublishLevel, PublishRegime,
    ReadinessConfig, ReadinessEvidence, ReconciliationOutcome, RuntimeOptions,
};

use super::policy::policy_effects;
use super::readiness::is_version_visible_with_backoff_and_events;
use super::reconcile::reconcile_ambiguous_upload;
use super::webhook::{WebhookEvent, maybe_send_event};
use super::{Reporter, SendReporter, drain_retry_waits};

use crate::plan::chunking::chunk_by_max_concurrent;

/// Result of publishing a single package (for parallel execution)
#[derive(Debug)]
pub(super) struct PackagePublishResult {
    pub(super) result: anyhow::Result<PackageReceipt>,
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
fn record_attempt_detail(
    st: &Arc<Mutex<ExecutionState>>,
    state_dir: &Path,
    detail: AttemptDetail,
) -> Result<()> {
    let mut state = st.lock().map_err(|_| {
        anyhow::anyhow!("execution state lock poisoned while recording attempt detail")
    })?;
    append_attempt_detail(&mut state, detail);
    state::save_state(state_dir, &state)
}

/// Emit a [`EventType::RetryBackoffStarted`] event + a human-readable warn
/// line through the Reporter, then `thread::sleep(delay)`. Used at every
/// retry-backoff site in the publish loop so operators never stare at a
/// silent CI log during the wait window. See #91.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_retry_backoff(
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
) {
    record_retry_backoff(
        event_log,
        events_path,
        pkg_label,
        attempt,
        max_attempts,
        delay,
        next_attempt_at,
        &reason,
        message,
    );
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
}

#[allow(clippy::too_many_arguments)]
fn record_retry_backoff(
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    pkg_label: &str,
    attempt: u32,
    max_attempts: u32,
    delay: std::time::Duration,
    next_attempt_at: DateTime<Utc>,
    reason: &ErrorClass,
    message: &str,
) {
    let Ok(mut log) = event_log.lock() else {
        return;
    };
    log.record(PublishEvent {
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
    });
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PublishWaiting {
            reason: "retry backoff".to_string(),
            delay_ms: delay.as_millis() as u64,
            until: next_attempt_at,
        },
        package: pkg_label.to_string(),
    });
    log.record(PublishEvent {
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
    });
    let _ = log.write_to_file(events_path);
    log.clear();
}

fn record_rate_limit_observed(
    event_log: &Arc<Mutex<events::EventLog>>,
    events_path: &Path,
    pkg_label: &str,
    is_new_crate: bool,
    retry_after: Option<std::time::Duration>,
    message: &str,
) {
    let Ok(mut log) = event_log.lock() else {
        return;
    };
    log.record(PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::RateLimitObserved {
            is_new_crate,
            retry_after_ms: retry_after.map(|delay| delay.as_millis() as u64),
            message: message.to_string(),
        },
        package: pkg_label.to_string(),
    });
    let _ = log.write_to_file(events_path);
    log.clear();
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
    events_path: &Path,
    reporter: &Arc<SendReporter>,
) {
    if let Err(err) = crate::state::reconciliation::write_report_from_events(
        state_dir,
        &ws.plan.plan_id,
        &ws.plan.registry,
        events_path,
    ) {
        reporter.warn(&format!("failed to write reconciliation report: {err}"));
    }
}

/// Publish a single package with retries (parallel-safe version)
#[allow(clippy::too_many_arguments)]
pub(super) fn publish_package(
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
    let key = pkg_key(&p.name, &p.version);
    let pkg_label = format!("{}@{}", p.name, p.version);
    let started_at = Utc::now();
    let start_instant = Instant::now();

    // Record package started event
    {
        let Ok(mut log) = event_log.lock() else {
            return poisoned_lock("event log");
        };
        log.record(PublishEvent {
            timestamp: started_at,
            event_type: EventType::PackageStarted {
                name: p.name.clone(),
                version: p.version.clone(),
            },
            package: pkg_label.clone(),
        });
        let _ = log.write_to_file(events_path);
        log.clear();
    }

    // Check if already published
    if let Ok(true) = reg.version_exists(&p.name, &p.version) {
        reporter.info(&format!(
            "{}@{}: already published (skipping)",
            p.name, p.version
        ));

        let skipped = PackageState::Skipped {
            reason: "already published".to_string(),
        };
        {
            let Ok(mut state) = st.lock() else {
                return poisoned_lock("execution state");
            };
            update_state_locked(&mut state, &key, skipped.clone());
            let _ = state::save_state(state_dir, &state);
        }

        // Event: PackageSkipped
        {
            let Ok(mut log) = event_log.lock() else {
                return poisoned_lock("event log");
            };
            log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PackageSkipped {
                    reason: "already published".to_string(),
                },
                package: pkg_label.clone(),
            });
            let _ = log.write_to_file(events_path);
            log.clear();
        }

        return PackagePublishResult {
            result: Ok(PackageReceipt {
                name: p.name.clone(),
                version: p.version.clone(),
                attempts: 0,
                state: skipped,
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

    reporter.info(&format!("{}@{}: publishing...", p.name, p.version));

    let mut attempt = 0u32;
    let mut last_err: Option<(ErrorClass, String)> = None;
    let mut attempt_evidence: Vec<AttemptEvidence> = Vec::new();
    let mut readiness_evidence: Vec<ReadinessEvidence> = Vec::new();
    let mut cargo_succeeded = false;

    // Check if resuming from Uploaded state (cargo publish succeeded previously)
    {
        let Ok(state) = st.lock() else {
            return poisoned_lock("execution state");
        };
        if let Some(pr) = state.packages.get(&key)
            && matches!(pr.state, PackageState::Uploaded)
        {
            cargo_succeeded = true;
        }
    }

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
    let ambiguous_prior: Option<String> = {
        let Ok(state) = st.lock() else {
            return poisoned_lock("execution state");
        };
        state.packages.get(&key).and_then(|pr| {
            if let PackageState::Ambiguous { message } = &pr.state {
                Some(message.clone())
            } else {
                None
            }
        })
    };

    if let Some(prior_reason) = ambiguous_prior {
        reporter.warn(&format!(
            "{}@{}: resume found ambiguous state ({}); reconciling against registry",
            p.name, p.version, prior_reason
        ));
        {
            let Ok(mut log) = event_log.lock() else {
                return poisoned_lock("event log");
            };
            log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PublishReconciling {
                    method: readiness_config.method,
                },
                package: pkg_label.clone(),
            });
        }

        let (outcome, _evidence) =
            reconcile_ambiguous_upload(reg, &p.name, &p.version, &readiness_config);

        {
            let Ok(mut log) = event_log.lock() else {
                return poisoned_lock("event log");
            };
            log.record(PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::PublishReconciled {
                    outcome: outcome.clone(),
                },
                package: pkg_label.clone(),
            });
            let _ = log.write_to_file(events_path);
            log.clear();
        }
        write_reconciliation_report_best_effort(state_dir, ws, events_path, reporter);
        let reconciliation_report_path = state::reconciliation_path(state_dir);

        match outcome {
            ReconciliationOutcome::Published { .. } => {
                {
                    let Ok(mut state) = st.lock() else {
                        return poisoned_lock("execution state");
                    };
                    update_state_locked(&mut state, &key, PackageState::Published);
                    let _ = state::save_state(state_dir, &state);
                }
                reporter.info(&format!(
                    "{}@{}: reconciliation outcome: Published; action: mark published and continue without republish (evidence: {})",
                    p.name,
                    p.version,
                    reconciliation_report_path.display()
                ));
                return PackagePublishResult {
                    result: Ok(PackageReceipt {
                        name: p.name.clone(),
                        version: p.version.clone(),
                        attempts: 0,
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
                {
                    let Ok(mut state) = st.lock() else {
                        return poisoned_lock("execution state");
                    };
                    update_state_locked(&mut state, &key, PackageState::Pending);
                    let _ = state::save_state(state_dir, &state);
                }
                reporter.info(&format!(
                    "{}@{}: reconciliation outcome: NotPublished; action: retry under publish policy (evidence: {})",
                    p.name,
                    p.version,
                    reconciliation_report_path.display()
                ));
                // Fall through to the normal retry loop below.
            }
            ReconciliationOutcome::StillUnknown { reason, .. } => {
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
                    result: Err(anyhow::anyhow!(
                        "{}@{}: resume reconciliation still inconclusive; operator action required. Prior reason: {}",
                        p.name,
                        p.version,
                        reason
                    )),
                };
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
        attempt += 1;
        {
            let Ok(mut state) = st.lock() else {
                return poisoned_lock("execution state");
            };
            if let Some(pr) = state.packages.get_mut(&key) {
                pr.attempts = attempt;
                pr.last_updated_at = Utc::now();
            }
            let _ = state::save_state(state_dir, &state);
        }

        let command = format!(
            "cargo publish -p {} --registry {}",
            p.name, ws.plan.registry.name
        );

        reporter.info(&format!(
            "{}@{}: attempt {}/{}",
            p.name, p.version, attempt, opts.max_attempts
        ));

        if !cargo_succeeded {
            // Event: PackageAttempted
            let attempt_started_at = Utc::now();
            {
                let Ok(mut log) = event_log.lock() else {
                    return poisoned_lock("event log");
                };
                log.record(PublishEvent {
                    timestamp: attempt_started_at,
                    event_type: EventType::PackageAttempted {
                        attempt,
                        command: command.clone(),
                    },
                    package: pkg_label.clone(),
                });
            }

            let out = match cargo::cargo_publish(
                &ws.workspace_root,
                &p.name,
                &ws.plan.registry.name,
                opts.allow_dirty,
                opts.no_verify,
                opts.output_lines,
                Some(opts.parallel.per_package_timeout),
            ) {
                Ok(o) => o,
                Err(e) => {
                    reporter.error(&format!(
                        "{}@{}: cargo publish failed to execute: {}",
                        p.name, p.version, e
                    ));
                    return PackagePublishResult { result: Err(e) };
                }
            };
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
                let _ = log.write_to_file(events_path);
                log.clear();
            }

            if out.exit_code == 0 && !out.timed_out {
                if let Err(e) = record_attempt_detail(
                    st,
                    state_dir,
                    AttemptDetail {
                        package: p.name.clone(),
                        version: p.version.clone(),
                        attempt,
                        max_attempts: opts.max_attempts,
                        started_at: attempt_started_at,
                        ended_at: attempt_ended_at,
                        error_class: None,
                        next_attempt_at: None,
                        redacted_message: None,
                    },
                ) {
                    return PackagePublishResult { result: Err(e) };
                }
                cargo_succeeded = true;
                // Persist Uploaded state so resume skips cargo publish
                {
                    let Ok(mut state) = st.lock() else {
                        return poisoned_lock("execution state");
                    };
                    update_state_locked(&mut state, &key, PackageState::Uploaded);
                    let _ = state::save_state(state_dir, &state);
                }
            } else {
                // Cargo failed, check registry
                reporter.warn(&format!(
                    "{}@{}: cargo publish failed (exit={}); checking registry...",
                    p.name, p.version, out.exit_code
                ));

                let failure_output = format!("{}\n{}", out.stderr_tail, out.stdout_tail);
                let (class, msg) = classify_cargo_failure(&out.stderr_tail, &out.stdout_tail);
                last_err = Some((class.clone(), msg.clone()));
                let mut attempt_detail = AttemptDetail {
                    package: p.name.clone(),
                    version: p.version.clone(),
                    attempt,
                    max_attempts: opts.max_attempts,
                    started_at: attempt_started_at,
                    ended_at: attempt_ended_at,
                    error_class: Some(class.clone()),
                    next_attempt_at: None,
                    redacted_message: Some(msg.clone()),
                };

                if reg.version_exists(&p.name, &p.version).unwrap_or(false) {
                    reporter.info(&format!(
                        "{}@{}: version is present on registry; treating as published",
                        p.name, p.version
                    ));

                    if let Err(e) = record_attempt_detail(st, state_dir, attempt_detail) {
                        return PackagePublishResult { result: Err(e) };
                    }
                    {
                        let Ok(mut state) = st.lock() else {
                            return poisoned_lock("execution state");
                        };
                        update_state_locked(&mut state, &key, PackageState::Published);
                        let _ = state::save_state(state_dir, &state);
                    }
                    last_err = None;
                    break;
                }

                // Event: PackageFailed
                {
                    let Ok(mut log) = event_log.lock() else {
                        return poisoned_lock("event log");
                    };
                    log.record(PublishEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::PackageFailed {
                            class: class.clone(),
                            message: msg.clone(),
                        },
                        package: pkg_label.clone(),
                    });
                }

                // On Ambiguous: never blind-retry. Reconcile against registry
                // truth first so we don't risk a duplicate upload. See #99.
                if class == ErrorClass::Ambiguous {
                    {
                        let Ok(mut log) = event_log.lock() else {
                            return poisoned_lock("event log");
                        };
                        log.record(PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PublishReconciling {
                                method: readiness_config.method,
                            },
                            package: pkg_label.clone(),
                        });
                    }
                    reporter.warn(&format!(
                        "{}@{}: cargo exit ambiguous; reconciling against registry",
                        p.name, p.version
                    ));

                    let (outcome, reconcile_evidence) =
                        reconcile_ambiguous_upload(reg, &p.name, &p.version, &readiness_config);

                    {
                        let Ok(mut log) = event_log.lock() else {
                            return poisoned_lock("event log");
                        };
                        log.record(PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PublishReconciled {
                                outcome: outcome.clone(),
                            },
                            package: pkg_label.clone(),
                        });
                        let _ = log.write_to_file(events_path);
                        log.clear();
                    }
                    write_reconciliation_report_best_effort(state_dir, ws, events_path, reporter);
                    let reconciliation_report_path = state::reconciliation_path(state_dir);

                    match outcome {
                        ReconciliationOutcome::Published { .. } => {
                            if let Err(e) =
                                record_attempt_detail(st, state_dir, attempt_detail.clone())
                            {
                                return PackagePublishResult { result: Err(e) };
                            }
                            reporter.info(&format!(
                                "{}@{}: reconciliation outcome: Published; registry shows version present; action: mark published and continue without retry (evidence: {})",
                                p.name,
                                p.version,
                                reconciliation_report_path.display()
                            ));
                            {
                                let Ok(mut state) = st.lock() else {
                                    return poisoned_lock("execution state");
                                };
                                update_state_locked(&mut state, &key, PackageState::Published);
                                let _ = state::save_state(state_dir, &state);
                            }
                            {
                                let Ok(mut log) = event_log.lock() else {
                                    return poisoned_lock("event log");
                                };
                                log.record(PublishEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::PackagePublished {
                                        duration_ms: start_instant.elapsed().as_millis() as u64,
                                    },
                                    package: pkg_label.clone(),
                                });
                                let _ = log.write_to_file(events_path);
                                log.clear();
                            }

                            // Preserve reconciliation evidence in the receipt.
                            // Do NOT emit PublishSucceeded webhook here Ã¢â‚¬â€ the
                            // end-of-function success path (below) handles that.
                            readiness_evidence = reconcile_evidence;
                            last_err = None;
                            break;
                        }
                        ReconciliationOutcome::NotPublished { .. } => {
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
                        }
                        ReconciliationOutcome::StillUnknown { reason, .. } => {
                            if let Err(e) =
                                record_attempt_detail(st, state_dir, attempt_detail.clone())
                            {
                                return PackagePublishResult { result: Err(e) };
                            }
                            let ambiguous_state = PackageState::Ambiguous {
                                message: reason.clone(),
                            };
                            {
                                let Ok(mut state) = st.lock() else {
                                    return poisoned_lock("execution state");
                                };
                                update_state_locked(&mut state, &key, ambiguous_state);
                                let _ = state::save_state(state_dir, &state);
                            }
                            {
                                let Ok(mut log) = event_log.lock() else {
                                    return poisoned_lock("event log");
                                };
                                let _ = log.write_to_file(events_path);
                                log.clear();
                            }
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
                                result: Err(anyhow::anyhow!(
                                    "{}@{}: reconciliation inconclusive: {}",
                                    p.name,
                                    p.version,
                                    reason
                                )),
                            };
                        }
                    }
                }

                match class {
                    ErrorClass::Permanent => {
                        if let Err(e) = record_attempt_detail(st, state_dir, attempt_detail) {
                            return PackagePublishResult { result: Err(e) };
                        }
                        let failed = PackageState::Failed {
                            class: class.clone(),
                            message: msg.clone(),
                        };
                        {
                            let Ok(mut state) = st.lock() else {
                                return poisoned_lock("execution state");
                            };
                            update_state_locked(&mut state, &key, failed);
                            let _ = state::save_state(state_dir, &state);
                        }
                        {
                            let Ok(mut log) = event_log.lock() else {
                                return poisoned_lock("event log");
                            };
                            let _ = log.write_to_file(events_path);
                            log.clear();
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
                        // returned NotPublished Ã¢â‚¬â€ registry confirms no
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
                                record_rate_limit_observed(
                                    event_log,
                                    events_path,
                                    &pkg_label,
                                    is_new_crate,
                                    retry_after_delay(&failure_output),
                                    &msg,
                                );
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
                            record_retry_backoff(
                                event_log,
                                events_path,
                                &pkg_label,
                                attempt,
                                opts.max_attempts,
                                delay,
                                next_attempt_at,
                                &class,
                                &msg,
                            );
                            if let Err(e) = record_attempt_detail(st, state_dir, attempt_detail) {
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
                        } else if let Err(e) = record_attempt_detail(st, state_dir, attempt_detail)
                        {
                            return PackagePublishResult { result: Err(e) };
                        }
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
        if let Err(e) = record_readiness_event(
            event_log,
            events_path,
            PublishEvent {
                timestamp: Utc::now(),
                event_type: EventType::ReadinessStarted {
                    method: readiness_config.method,
                },
                package: pkg_label.clone(),
            },
        ) {
            return PackagePublishResult { result: Err(e) };
        }
        let mut emit_readiness_event =
            |event| record_readiness_event(event_log, events_path, event);
        let verify_result = is_version_visible_with_backoff_and_events(
            reg,
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
                                attempts: readiness_evidence.len() as u32,
                            },
                            package: pkg_label.clone(),
                        },
                    ) {
                        return PackagePublishResult { result: Err(e) };
                    }
                    {
                        let Ok(mut state) = st.lock() else {
                            return poisoned_lock("execution state");
                        };
                        update_state_locked(&mut state, &key, PackageState::Published);
                        let _ = state::save_state(state_dir, &state);
                    }
                    last_err = None;

                    // Event: PackagePublished
                    {
                        let Ok(mut log) = event_log.lock() else {
                            return poisoned_lock("event log");
                        };
                        log.record(PublishEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::PackagePublished {
                                duration_ms: start_instant.elapsed().as_millis() as u64,
                            },
                            package: pkg_label.clone(),
                        });
                        let _ = log.write_to_file(events_path);
                        log.clear();
                    }

                    // Send webhook notification: package succeeded
                    maybe_send_event(
                        &opts.webhook,
                        WebhookEvent::PublishSucceeded {
                            plan_id: ws.plan.plan_id.clone(),
                            package_name: p.name.clone(),
                            package_version: p.version.clone(),
                            duration_ms: start_instant.elapsed().as_millis() as u64,
                        },
                    );

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
                    let delay = backoff_delay(
                        opts.base_delay,
                        opts.max_delay,
                        attempt,
                        opts.retry_strategy,
                        opts.retry_jitter,
                    );
                    let next_attempt_at = retry_next_attempt_at(delay);
                    emit_retry_backoff(
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
                    );
                }
            }
            Err(_) => {
                let message = "readiness check failed";
                last_err = Some((ErrorClass::Ambiguous, message.to_string()));
                let delay = backoff_delay(
                    opts.base_delay,
                    opts.max_delay,
                    attempt,
                    opts.retry_strategy,
                    opts.retry_jitter,
                );
                let next_attempt_at = retry_next_attempt_at(delay);
                emit_retry_backoff(
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
                );
            }
        }
    }

    // If package is still Uploaded (loop didn't run or readiness never checked), force a final check
    if last_err.is_none() {
        let current_state = st
            .lock()
            .unwrap()
            .packages
            .get(&key)
            .map(|p| p.state.clone());
        if matches!(current_state, Some(PackageState::Uploaded)) {
            if reg.version_exists(&p.name, &p.version).unwrap_or(false) {
                {
                    let Ok(mut state) = st.lock() else {
                        return poisoned_lock("execution state");
                    };
                    update_state_locked(&mut state, &key, PackageState::Published);
                    let _ = state::save_state(state_dir, &state);
                }

                // Send webhook notification: package succeeded
                maybe_send_event(
                    &opts.webhook,
                    WebhookEvent::PublishSucceeded {
                        plan_id: ws.plan.plan_id.clone(),
                        package_name: p.name.clone(),
                        package_version: p.version.clone(),
                        duration_ms: start_instant.elapsed().as_millis() as u64,
                    },
                );
            } else {
                last_err = Some((
                    ErrorClass::Ambiguous,
                    "package was uploaded but not confirmed visible on registry".into(),
                ));
            }
        }
    }

    let finished_at = Utc::now();
    let duration_ms = start_instant.elapsed().as_millis();

    if let Some((class, msg)) = last_err {
        // Final chance: maybe it eventually showed up.
        if reg.version_exists(&p.name, &p.version).unwrap_or(false) {
            {
                let Ok(mut state) = st.lock() else {
                    return poisoned_lock("execution state");
                };
                update_state_locked(&mut state, &key, PackageState::Published);
                let _ = state::save_state(state_dir, &state);
            }

            // Send webhook notification: package succeeded
            maybe_send_event(
                &opts.webhook,
                WebhookEvent::PublishSucceeded {
                    plan_id: ws.plan.plan_id.clone(),
                    package_name: p.name.clone(),
                    package_version: p.version.clone(),
                    duration_ms: duration_ms as u64,
                },
            );

            return PackagePublishResult {
                result: Ok(PackageReceipt {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    attempts: st
                        .lock()
                        .unwrap()
                        .packages
                        .get(&key)
                        .map_or(0, |p| p.attempts),
                    state: PackageState::Published,
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
            };
        } else {
            let error_class_str = format!("{:?}", class);
            let failed = PackageState::Failed {
                class,
                message: msg.clone(),
            };
            {
                let Ok(mut state) = st.lock() else {
                    return poisoned_lock("execution state");
                };
                update_state_locked(&mut state, &key, failed);
                let _ = state::save_state(state_dir, &state);
            }

            // Event: PackageFailed (final)
            {
                let Ok(mut log) = event_log.lock() else {
                    return poisoned_lock("event log");
                };
                log.record(PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::PackageFailed {
                        class: ErrorClass::Ambiguous,
                        message: msg.clone(),
                    },
                    package: pkg_label,
                });
                let _ = log.write_to_file(events_path);
                log.clear();
            }

            // Send webhook notification: package failed
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

    // Send webhook notification: package succeeded
    maybe_send_event(
        &opts.webhook,
        WebhookEvent::PublishSucceeded {
            plan_id: ws.plan.plan_id.clone(),
            package_name: p.name.clone(),
            package_version: p.version.clone(),
            duration_ms: duration_ms as u64,
        },
    );

    PackagePublishResult {
        result: Ok(PackageReceipt {
            name: p.name.clone(),
            version: p.version.clone(),
            attempts: st
                .lock()
                .unwrap()
                .packages
                .get(&key)
                .map_or(0, |p| p.attempts),
            state: PackageState::Published,
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

/// Publish packages in a single level in parallel
#[allow(clippy::too_many_arguments)]
pub(super) fn run_publish_level(
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

    // Process packages in batches limited by max_concurrent
    for chunk in chunk_by_max_concurrent(&level.packages, max_concurrent) {
        let mut handles: Vec<std::thread::JoinHandle<PackagePublishResult>> = Vec::new();

        // Start all packages in this chunk
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

            handles.push(handle);
        }

        while handles.iter().any(|handle| !handle.is_finished()) {
            drain_retry_waits(reporter, send_reporter.as_ref());
            thread::sleep(Duration::from_millis(25));
        }
        drain_retry_waits(reporter, send_reporter.as_ref());

        // Wait for all packages in this chunk to complete, collecting all results
        for handle in handles {
            let result = handle
                .join()
                .map_err(|_| anyhow::anyhow!("publish thread panicked"))?;
            match result.result {
                Ok(receipt) => all_receipts.push(receipt),
                Err(e) => errors.push(format!("{e:#}")),
            }
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
