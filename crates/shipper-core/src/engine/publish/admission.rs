//! Read retained publication authority before any new execution effects.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::cli_bridge::{RunLiveness, RunObservation};
use crate::engine::{
    PublishAdmissionError, PublishCleanupIncompleteError, reject_exhausted_resume_retry_ceiling,
};
use crate::lock::{LockContents, LockReservation};
use crate::plan::PlannedWorkspace;
use crate::state::upload_evidence::UploadWitness;
use crate::state::{consistency, events, execution_state as state, run_observation};
use crate::types::{
    EventType, ExecutionState, PackageState, PublishEvent, ReconciliationReport, RuntimeOptions,
};

/// This is an early diagnostic only. Its result makes the state requirement
/// sticky: a caller that saw retained state may not initialize a new run if
/// that state disappears before it obtains ownership.
pub(in crate::engine) fn precheck(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    require_existing: bool,
) -> Result<bool> {
    super::bootstrap::validate_resume_target(ws, opts)?;
    load_retained_state(ws, opts, state_dir, require_existing, None)
        .map(|state| state.is_some())
        .context(PublishAdmissionError)
}

/// Own the invocation key, then reread and admit current durable evidence.
/// The returned reservation has not replaced any predecessor JSON. A refusal
/// drops only the OS guard, preserving all retained evidence bytes.
pub(in crate::engine) fn reserve_and_admit(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    require_existing: bool,
) -> Result<(LockReservation, Option<ExecutionState>)> {
    (|| -> Result<_> {
        let reservation = LockReservation::acquire(
            state_dir,
            Some(&ws.workspace_root),
            opts.lock_timeout,
            opts.force,
        )
        .context("failed to reserve publish ownership")?;
        let retained = load_retained_state(
            ws,
            opts,
            state_dir,
            require_existing,
            Some(reservation.prior_contents()),
        )?;
        Ok((reservation, retained))
    })()
    .context(PublishAdmissionError)
}

fn load_retained_state(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    require_existing: bool,
    prior: Option<&LockContents>,
) -> Result<Option<ExecutionState>> {
    let (existing, log) = load_cleanup_checked_evidence(state_dir)?;
    let Some(existing) = existing else {
        if require_existing {
            bail!(
                "no existing state found in {}; retained state is required, so preserve and inspect the evidence before retrying",
                state_dir.display()
            );
        }
        ensure_no_retained_execution(state_dir, &log)?;
        return Ok(None);
    };
    super::bootstrap::validate_existing_plan_id(&existing, ws, opts)?;
    let observation = match prior {
        Some(prior) => {
            // During precheck another publisher can still be between writes.
            // Judge its retained attempt only once we own the reservation.
            ensure_upload_checkpoint_is_safe(ws, &existing, &log)?;
            run_observation::observe_run_with_prior_lock(state_dir, prior)?
        }
        None => run_observation::observe_run(state_dir, Some(&ws.workspace_root))?,
    };
    let reconciliation_path = state::reconciliation_path(state_dir);
    let reconciliation = if reconciliation_path.exists() {
        Some(
            std::fs::read_to_string(&reconciliation_path)
                .with_context(|| {
                    format!(
                        "failed to read reconciliation evidence {}",
                        reconciliation_path.display()
                    )
                })
                .and_then(|raw| {
                    serde_json::from_str::<ReconciliationReport>(&raw).with_context(|| {
                        format!(
                            "failed to parse reconciliation evidence {}",
                            reconciliation_path.display()
                        )
                    })
                })?,
        )
    } else {
        None
    };
    let derived_recoverable =
        consistency::has_not_published_retryable_posture(&existing, reconciliation.as_ref());
    let marker_present = matches!(&observation, RunObservation::Stopped { .. });
    let events_path = events::events_path(state_dir);
    let event_candidate = consistency::has_current_not_published_retryable_failure(&events_path)?;
    if derived_recoverable || marker_present || event_candidate {
        match observation {
            RunObservation::Stopped {
                liveness: None | Some(RunLiveness::NotLive),
                ..
            } => {}
            RunObservation::Stopped {
                liveness: Some(RunLiveness::Live),
                ..
            } => bail!("controlled-stop publisher is still live; publication admission is blocked"),
            RunObservation::Stopped {
                liveness: Some(RunLiveness::Unknown(reason)),
                ..
            } => bail!(
                "controlled-stop lock liveness is inconclusive ({reason:?}); publication admission is blocked"
            ),
            _ => bail!(
                "retryable NotPublished posture lacks the current controlled-stop marker; publication admission is blocked"
            ),
        }
        if state::receipt_path(state_dir).exists() {
            bail!("controlled-stop evidence unexpectedly includes a receipt");
        }
        consistency::verify_controlled_stop_consistency(
            &events_path,
            &existing,
            reconciliation.as_ref(),
        )
        .context("controlled-stop evidence does not authorize publication")?;
        // Keep the existing cumulative, global ceiling and selected scheduler
        // scope. Ambiguous, Permanent and Uploaded recovery keep their own paths.
        reject_exhausted_resume_retry_ceiling(ws, opts, &existing)?;
    }
    Ok(Some(existing))
}

/// A split write can leave a readable snapshot behind its latest Cargo
/// evidence. Accept compatible snapshots and keep positive witnesses through
/// later diagnostic and retry records, without reconstructing the projection.
fn ensure_upload_checkpoint_is_safe(
    ws: &PlannedWorkspace,
    state: &ExecutionState,
    log: &events::EventLog,
) -> Result<()> {
    let mut witnesses = BTreeMap::<&str, UploadWitness>::new();
    for event in log.all_events() {
        let witness = witnesses.entry(&event.package).or_default();
        *witness = witness.observe(&event.event_type);
    }

    for package in &ws.plan.packages {
        let key = crate::runtime::execution::pkg_key(&package.name, &package.version);
        let witness = witnesses.get(key.as_str()).copied().unwrap_or_default();
        ensure!(
            witness.permits(state.packages.get(&key), &state.attempt_history),
            "{key}: {}",
            match witness {
                UploadWitness::NotLaunched(_) =>
                    "retained events prove Cargo did not launch, but state.json lacks the matching attempt; preserve both files and reconcile the state projection before retrying",
                _ =>
                    "retained events show an unresolved Cargo upload or durable upload outcome that state.json cannot safely resume; preserve both files and reconcile the prior upload before retrying",
            }
        );
    }
    Ok(())
}

fn ensure_no_retained_execution(state_dir: &Path, log: &events::EventLog) -> Result<()> {
    let has_execution = log.all_events().iter().any(|event| {
        !matches!(
            event.event_type,
            EventType::PlanCreated { .. }
                | EventType::AuthEvidenceRecorded { .. }
                | EventType::RegistryPolicyApplied { .. }
                | EventType::PackageYanked { .. }
                | EventType::PreflightStarted
                | EventType::PreflightWorkspaceVerify { .. }
                | EventType::PreflightNewCrateDetected { .. }
                | EventType::PreflightOwnershipCheck { .. }
                | EventType::PreflightComplete { .. }
                | EventType::RehearsalStarted { .. }
                | EventType::RehearsalPackagePublished { .. }
                | EventType::RehearsalPackageFailed { .. }
                | EventType::RehearsalComplete { .. }
                | EventType::RehearsalSmokeCheckStarted { .. }
                | EventType::RehearsalSmokeCheckSucceeded { .. }
                | EventType::RehearsalSmokeCheckFailed { .. }
        )
    });
    ensure!(
        !has_execution
            && !state::receipt_path(state_dir).exists()
            && !state::reconciliation_path(state_dir).exists(),
        "publish state is missing while retained execution evidence exists in {}; preserve that evidence and restore the required state before retrying",
        state_dir.display()
    );
    Ok(())
}

/// A direct parallel caller may supply an initial projection, but memory alone
/// cannot authorize resuming prior work when its durable state is missing.
pub(in crate::engine) fn validate_fresh_parallel_state(
    ws: &PlannedWorkspace,
    supplied: &ExecutionState,
) -> Result<()> {
    ensure!(
        supplied.plan_id == ws.plan.plan_id
            && supplied.registry.name == ws.plan.registry.name
            && supplied.registry.api_base == ws.plan.registry.api_base
            && supplied.registry.index_base == ws.plan.registry.index_base
            && supplied.attempt_history.is_empty()
            && supplied.packages.len() == ws.plan.packages.len()
            && ws.plan.packages.iter().all(|package| {
                supplied
                    .packages
                    .get(&crate::runtime::execution::pkg_key(
                        &package.name,
                        &package.version,
                    ))
                    .is_some_and(|progress| {
                        progress.name == package.name
                            && progress.version == package.version
                            && progress.attempts == 0
                            && matches!(progress.state, PackageState::Pending)
                    })
            }),
        "direct parallel publication without durable state requires an unattempted initial projection for the current plan and registry"
    );
    Ok(())
}

/// Process cleanup is a run-wide local safety condition, not registry truth.
/// Keep this inside the retained-admission owner so all public entry points
/// inspect the same state and authoritative event evidence before mutation.
pub(in crate::engine) fn validate_cleanup_safety(state_dir: &Path) -> Result<()> {
    load_cleanup_checked_evidence(state_dir).map(|_| ())
}

/// Rehearsal uses the same ownership key, but has no normal publish projection
/// to admit. Recheck the local cleanup condition after reservation and before
/// replacing predecessor metadata or emitting any rehearsal event.
pub(in crate::engine) fn reserve_rehearsal(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
) -> Result<LockReservation> {
    (|| -> Result<_> {
        let reservation = LockReservation::acquire(
            state_dir,
            Some(&ws.workspace_root),
            opts.lock_timeout,
            opts.force,
        )
        .context("failed to reserve rehearsal ownership")?;
        validate_cleanup_safety(state_dir)?;
        Ok(reservation)
    })()
    .context(PublishAdmissionError)
}

pub(in crate::engine) fn ensure_no_cleanup_hold(state: &ExecutionState) -> Result<()> {
    if let Some((package, _)) = state
        .packages
        .iter()
        .find(|(_, progress)| matches!(progress.state, PackageState::CleanupIncomplete { .. }))
    {
        return Err(PublishCleanupIncompleteError {
            message: format!("{package}: retained process cleanup is incomplete; publication is blocked; preserve the evidence and inspect the owned processes before any new publication"),
        }.into());
    }
    Ok(())
}

fn load_cleanup_checked_evidence(
    state_dir: &Path,
) -> Result<(Option<ExecutionState>, events::EventLog)> {
    // The underlying event reader includes malformed line content in its
    // context. Do not echo arbitrary retained payloads through this admission
    // boundary, including when the projection is missing or unreadable.
    let retained = state::state_path(state_dir)
        .try_exists()
        .map_err(anyhow::Error::from)
        .and_then(|_| state::load_state(state_dir))
        .map_err(|error| {
            // Keep the actionable error category without forwarding serde's
            // arbitrary retained values (for example an unknown enum variant).
            if error.is::<serde_json::Error>() {
                anyhow::anyhow!(
                    "failed to parse state JSON; cannot inspect retained publication state; preserve the evidence before retrying"
                )
            } else {
                anyhow::anyhow!(
                    "cannot inspect retained publication state; preserve the evidence before retrying"
                )
            }
        });
    if let Ok(Some(state)) = &retained {
        ensure_no_cleanup_hold(state)?;
    }
    let events_path = events::events_path(state_dir);
    let log = events_path
        .try_exists()
        .map_err(anyhow::Error::from)
        .and_then(|_| events::EventLog::read_from_file(&events_path));
    let held_package = match &log {
        Ok(log) => log
            .all_events()
            .iter()
            .find(|event| matches!(event.event_type, EventType::PackageCleanupIncomplete { .. }))
            .map(|event| event.package.clone()),
        Err(_) => recognizable_cleanup_hold(&events_path),
    };
    if let Some(package) = held_package {
        return Err(PublishCleanupIncompleteError {
            message: format!("{package}: retained process-cleanup event blocks publication, including with missing or stale state; preserve the evidence and inspect the owned processes before any new publication"),
        }.into());
    }
    let log = log.map_err(|_| {
        anyhow::anyhow!(
            "cannot inspect retained publication events; preserve the evidence before retrying"
        )
    })?;
    // A readable hold event takes precedence over a malformed projection;
    // otherwise any unreadable artifact still refuses admission.
    Ok((retained?, log))
}

/// A positive, fully typed hold record still refuses cleanup when a different
/// line is corrupt. This scan never authorizes publication or repairs history:
/// without a recognized hold the original whole-log read error still wins.
fn recognizable_cleanup_hold(events_path: &Path) -> Option<String> {
    let mut reader = BufReader::new(std::fs::File::open(events_path).ok()?);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        if let Ok(event) = serde_json::from_slice::<PublishEvent>(&line)
            && matches!(event.event_type, EventType::PackageCleanupIncomplete { .. })
        {
            return Some(event.package);
        }
    }
}
