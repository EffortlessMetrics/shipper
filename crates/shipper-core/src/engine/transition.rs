//! Controlled durable package transitions.
//!
//! `events.jsonl` is authoritative, so a transition records the domain event
//! before persisting the new state projection. Callers receive the first
//! persistence error instead of silently continuing with a partially recorded
//! transition.

use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::state::events::EventLog;
use crate::state::execution_state;
use crate::types::{AttemptDetail, ExecutionState, PackageState, PublishEvent};

/// Apply one package transition through the shared event/state boundary.
///
/// The state is cloned before mutation. This keeps the caller's in-memory
/// projection unchanged when the event cannot be appended. If state
/// persistence fails after the event is written, the event remains the
/// authoritative record and finalization/rebuild can surface the drift.
pub(crate) fn commit(
    state: &mut ExecutionState,
    state_dir: &Path,
    event_log: &mut EventLog,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
    event: PublishEvent,
) -> Result<()> {
    if event.package != key {
        bail!(
            "package transition key '{}' does not match event package '{}'",
            key,
            event.package
        );
    }

    event_log.record(event);
    persist(
        state,
        state_dir,
        event_log,
        events_path,
        key,
        Some(new_state),
        None,
        None,
    )
}

/// Record a package attempt before Cargo is invoked and project the attempt
/// counter only after the event is durable.
pub(crate) fn commit_attempt(
    state: &mut ExecutionState,
    state_dir: &Path,
    event_log: &mut EventLog,
    events_path: &Path,
    key: &str,
    attempt: u32,
    event: PublishEvent,
) -> Result<()> {
    if event.package != key {
        bail!(
            "package attempt key '{}' does not match event package '{}'",
            key,
            event.package
        );
    }

    // Admission is this critical section, not a separate earlier check. A
    // cleanup hold observed by any peer blocks every later attempt event.
    super::publish::admission::ensure_no_cleanup_hold(state)?;
    crate::ops::process::interruption::check()?;

    event_log.record(event);
    persist(
        state,
        state_dir,
        event_log,
        events_path,
        key,
        None,
        Some(attempt),
        None,
    )
}

/// Apply a package transition and persist the completed attempt detail at the
/// same durable boundary.
///
/// Attempt details are operator-facing projection data. Keeping them in this
/// boundary prevents a successful terminal transition from being written by
/// one path while its matching attempt timeline is written by another.
pub(crate) fn commit_with_attempt_detail(
    state: &mut ExecutionState,
    state_dir: &Path,
    event_log: &mut EventLog,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
    event: PublishEvent,
    detail: AttemptDetail,
) -> Result<()> {
    validate_attempt_detail(key, &detail)?;
    if event.package != key {
        bail!(
            "package transition key '{}' does not match event package '{}'",
            key,
            event.package
        );
    }

    event_log.record(event);
    persist(
        state,
        state_dir,
        event_log,
        events_path,
        key,
        Some(new_state),
        None,
        Some(detail),
    )
}

/// Persist a transition whose domain event has already been recorded in the
/// in-memory log by the caller.
///
/// This is used when a transition is preceded by one or more explanatory
/// events, such as reconciliation. The most recently recorded event must be
/// for the same package; the boundary still writes the event log before the
/// state projection and only updates the caller's state after both succeed.
pub(crate) fn commit_pending(
    state: &mut ExecutionState,
    state_dir: &Path,
    event_log: &mut EventLog,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
) -> Result<()> {
    let event = event_log
        .all_events()
        .last()
        .with_context(|| format!("missing domain event for package transition: {key}"))?;
    if event.package != key {
        bail!(
            "package transition key '{}' does not match pending event package '{}'",
            key,
            event.package
        );
    }

    persist(
        state,
        state_dir,
        event_log,
        events_path,
        key,
        Some(new_state),
        None,
        None,
    )
}

/// Complete a transition whose explanatory events are already buffered and
/// persist the matching attempt detail through the same boundary.
pub(crate) fn commit_pending_with_attempt_detail(
    state: &mut ExecutionState,
    state_dir: &Path,
    event_log: &mut EventLog,
    events_path: &Path,
    key: &str,
    new_state: PackageState,
    detail: AttemptDetail,
) -> Result<()> {
    validate_attempt_detail(key, &detail)?;
    let event = event_log
        .all_events()
        .last()
        .with_context(|| format!("missing domain event for package transition: {key}"))?;
    if event.package != key {
        bail!(
            "package transition key '{}' does not match pending event package '{}'",
            key,
            event.package
        );
    }

    persist(
        state,
        state_dir,
        event_log,
        events_path,
        key,
        Some(new_state),
        None,
        Some(detail),
    )
}

/// Flush buffered explanatory events while appending attempt detail without
/// changing the package state projection. This is used for retry scheduling,
/// where the package remains pending between attempts.
#[cfg(test)]
pub(crate) fn commit_attempt_detail_pending(
    state: &mut ExecutionState,
    state_dir: &Path,
    event_log: &mut EventLog,
    events_path: &Path,
    key: &str,
    detail: AttemptDetail,
) -> Result<()> {
    validate_attempt_detail(key, &detail)?;
    if let Some(event) = event_log.all_events().last()
        && event.package != key
    {
        bail!(
            "attempt detail key '{}' does not match pending event package '{}'",
            key,
            event.package
        );
    }

    persist(
        state,
        state_dir,
        event_log,
        events_path,
        key,
        None,
        None,
        Some(detail),
    )
}

fn validate_attempt_detail(key: &str, detail: &AttemptDetail) -> Result<()> {
    let detail_key = format!("{}@{}", detail.package, detail.version);
    if detail_key != key {
        bail!(
            "package transition key '{}' does not match attempt detail package '{}'",
            key,
            detail_key
        );
    }
    Ok(())
}

fn persist(
    state: &mut ExecutionState,
    state_dir: &Path,
    event_log: &mut EventLog,
    events_path: &Path,
    key: &str,
    new_state: Option<PackageState>,
    attempt: Option<u32>,
    attempt_detail: Option<AttemptDetail>,
) -> Result<()> {
    let mut next_state = state.clone();
    let progress = next_state
        .packages
        .get_mut(key)
        .with_context(|| format!("missing package progress for transition: {key}"))?;
    if let Some(new_state) = new_state {
        progress.state = new_state;
    }
    if let Some(attempt) = attempt {
        progress.attempts = attempt;
    }
    if let Some(detail) = attempt_detail {
        crate::state::record_attempt_detail(&mut next_state.attempt_history, detail);
    }
    progress.last_updated_at = Utc::now();
    next_state.updated_at = Utc::now();

    event_log
        .write_to_file(events_path)
        .with_context(|| format!("failed to persist package transition event for {key}"))?;
    event_log.clear();

    execution_state::save_state(state_dir, &next_state)
        .with_context(|| format!("failed to persist package transition state for {key}"))?;
    *state = next_state;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs::File;

    use anyhow::Context;
    use chrono::Utc;
    use tempfile::tempdir;

    use super::{
        commit, commit_attempt, commit_attempt_detail_pending, commit_pending,
        commit_pending_with_attempt_detail, commit_with_attempt_detail,
    };
    use crate::state::events::{EventLog, events_path};
    use crate::types::{
        AttemptDetail, EventType, ExecutionState, PackageProgress, PackageState, PublishEvent,
        Registry,
    };

    fn state() -> ExecutionState {
        let now = Utc::now();
        ExecutionState {
            state_version: "shipper.state.v1".to_string(),
            plan_id: "plan-transition-test".to_string(),
            registry: Registry::crates_io(),
            created_at: now,
            updated_at: now,
            attempt_history: Vec::new(),
            packages: BTreeMap::from([(
                "demo@1.0.0".to_string(),
                PackageProgress {
                    name: "demo".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 0,
                    state: PackageState::Pending,
                    last_updated_at: now,
                },
            )]),
        }
    }

    fn published_event() -> PublishEvent {
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackagePublished { duration_ms: 7 },
            package: "demo@1.0.0".to_string(),
        }
    }

    fn attempted_event(attempt: u32) -> PublishEvent {
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageAttempted {
                attempt,
                command: "cargo publish -p demo".to_string(),
                max_attempts: attempt,
            },
            package: "demo@1.0.0".to_string(),
        }
    }

    #[test]
    #[serial_test::serial]
    fn file_sync_orders_events_before_projection_and_preserves_failed_commit() -> anyhow::Result<()>
    {
        use std::sync::{Arc, Mutex};

        for failed_file in [None, Some("events.jsonl"), Some("state.tmp")] {
            let dir = tempdir()?;
            let events = events_path(dir.path());
            let mut current = state();
            crate::state::execution_state::save_state(dir.path(), &current)?;
            let state_path = crate::state::execution_state::state_path(dir.path());
            let prior_bytes = std::fs::read(&state_path)?;
            let mut log = EventLog::new();
            let order = Arc::new(Mutex::new(Vec::new()));
            let observed_order = Arc::clone(&order);
            let observed_state = state_path.clone();
            let observed_prior = prior_bytes.clone();
            let fault = crate::state::file_sync_test_support::observe(dir.path(), move |path| {
                let name = path.file_name().unwrap().to_str().unwrap();
                observed_order.lock().unwrap().push(name.to_string());
                assert_eq!(std::fs::read(&observed_state).unwrap(), observed_prior);
                if name == "events.jsonl" {
                    let written = EventLog::read_from_file(path).unwrap();
                    assert_eq!(written.len(), 1, "the event was flushed before sync");
                    assert!(matches!(
                        written.all_events()[0].event_type,
                        EventType::PackageUploaded
                    ));
                } else {
                    assert_eq!(name, "state.tmp");
                    let written: ExecutionState =
                        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                    assert_eq!(written.packages["demo@1.0.0"].state, PackageState::Uploaded);
                }
                if failed_file == Some(name) {
                    return Err(std::io::Error::other(
                        "injected transition file-sync failure",
                    ));
                }
                Ok(())
            });

            let result = commit(
                &mut current,
                dir.path(),
                &mut log,
                &events,
                "demo@1.0.0",
                PackageState::Uploaded,
                PublishEvent {
                    event_type: EventType::PackageUploaded,
                    ..published_event()
                },
            );
            drop(fault);

            let expected_order = if failed_file == Some("events.jsonl") {
                vec!["events.jsonl"]
            } else {
                vec!["events.jsonl", "state.tmp"]
            };
            assert_eq!(*order.lock().unwrap(), expected_order);
            let retained_events = std::fs::read(&events)?;
            assert_eq!(EventLog::read_from_file(&events)?.len(), 1);
            if let Some(failed_file) = failed_file {
                let error = result.expect_err("required sync failure must stop the transition");
                assert!(format!("{error:#}").contains(failed_file));
                assert!(format!("{error:#}").contains("injected transition file-sync failure"));
                assert!(error.downcast_ref::<std::io::Error>().is_some());
                assert_eq!(current.packages["demo@1.0.0"].state, PackageState::Pending);
                assert_eq!(std::fs::read(&state_path)?, prior_bytes);
                let loaded = crate::state::execution_state::load_state(dir.path())?.unwrap();
                assert_eq!(loaded.packages["demo@1.0.0"].state, PackageState::Pending);
                if failed_file == "events.jsonl" {
                    assert_eq!(log.len(), 1, "event sync failure retains the pending batch");
                    assert!(!state_path.with_extension("tmp").exists());
                } else {
                    assert!(
                        log.is_empty(),
                        "successful event sync clears the pending batch"
                    );
                    assert!(state_path.with_extension("tmp").is_file());
                    let rebuilt = crate::state::rebuild::rebuild_state_file_from_events(
                        dir.path(),
                        crate::state::rebuild::StateRebuildOptions::new(current.registry.clone())
                            .with_fallback_plan_id(current.plan_id.clone()),
                    )?;
                    assert_eq!(rebuilt.packages["demo@1.0.0"].state, PackageState::Uploaded);
                    assert_eq!(
                        std::fs::read(&events)?,
                        retained_events,
                        "rebuild never rewrites truth"
                    );
                }
            } else {
                result?;
                assert!(log.is_empty());
                assert_eq!(current.packages["demo@1.0.0"].state, PackageState::Uploaded);
                assert!(!state_path.with_extension("tmp").exists());
                let loaded = crate::state::execution_state::load_state(dir.path())?.unwrap();
                assert_eq!(loaded.packages["demo@1.0.0"].state, PackageState::Uploaded);
            }
        }
        Ok(())
    }

    fn attempt_detail() -> AttemptDetail {
        let now = Utc::now();
        AttemptDetail {
            package: "demo".to_string(),
            version: "1.0.0".to_string(),
            attempt: 1,
            max_attempts: 3,
            started_at: now,
            ended_at: now,
            error_class: None,
            next_attempt_at: None,
            redacted_message: None,
        }
    }

    #[test]
    fn commit_persists_event_before_state_projection() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let original = state.clone();
        let mut log = EventLog::new();

        commit(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
            published_event(),
        )?;

        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Published);
        assert!(log.all_events().is_empty());
        let persisted = EventLog::read_from_file(&events)?;
        assert!(matches!(
            persisted
                .all_events()
                .first()
                .map(|event| &event.event_type),
            Some(EventType::PackagePublished { duration_ms: 7 })
        ));
        let saved = crate::state::execution_state::load_state(dir.path())?
            .context("transition test state was not persisted")?;
        assert_eq!(saved.packages["demo@1.0.0"].state, PackageState::Published);
        assert_eq!(original.packages["demo@1.0.0"].state, PackageState::Pending);
        Ok(())
    }

    #[test]
    fn commit_rejects_an_event_for_a_different_package() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();
        let err = commit(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
            PublishEvent {
                package: "other@1.0.0".to_string(),
                ..published_event()
            },
        )
        .expect_err("mismatched transition must fail");

        assert!(err.to_string().contains("does not match event package"));
        assert!(log.all_events().is_empty());
        assert!(!events.exists());
        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Pending);
        Ok(())
    }

    #[test]
    fn commit_attempt_persists_event_before_attempt_projection() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();

        commit_attempt(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            2,
            attempted_event(2),
        )?;

        assert_eq!(state.packages["demo@1.0.0"].attempts, 2);
        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Pending);
        assert!(log.all_events().is_empty());
        assert!(matches!(
            EventLog::read_from_file(&events)?
                .all_events()
                .first()
                .map(|event| &event.event_type),
            Some(EventType::PackageAttempted { attempt: 2, .. })
        ));
        Ok(())
    }

    #[test]
    fn commit_with_attempt_detail_persists_event_and_timeline_together() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();

        commit_with_attempt_detail(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
            published_event(),
            attempt_detail(),
        )?;

        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Published);
        assert_eq!(state.attempt_history.len(), 1);
        assert_eq!(state.attempt_history[0].attempt, 1);
        assert_eq!(EventLog::read_from_file(&events)?.len(), 1);
        let saved = crate::state::execution_state::load_state(dir.path())?
            .context("combined transition state was not persisted")?;
        assert_eq!(saved.attempt_history, state.attempt_history);
        Ok(())
    }

    #[test]
    fn commit_pending_with_attempt_detail_projects_buffered_events_and_timeline()
    -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();
        log.record(published_event());

        commit_pending_with_attempt_detail(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
            attempt_detail(),
        )?;

        assert!(log.all_events().is_empty());
        assert_eq!(state.attempt_history.len(), 1);
        assert_eq!(EventLog::read_from_file(&events)?.len(), 1);
        Ok(())
    }

    #[test]
    fn commit_attempt_detail_pending_flushes_retry_events_without_state_change()
    -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();
        log.record(attempted_event(1));

        commit_attempt_detail_pending(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            attempt_detail(),
        )?;

        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Pending);
        assert_eq!(state.packages["demo@1.0.0"].attempts, 0);
        assert_eq!(state.attempt_history.len(), 1);
        assert!(matches!(
            EventLog::read_from_file(&events)?
                .all_events()
                .first()
                .map(|event| &event.event_type),
            Some(EventType::PackageAttempted { attempt: 1, .. })
        ));
        Ok(())
    }

    #[test]
    fn commit_pending_persists_the_buffered_event_before_state_projection() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();
        log.record(published_event());

        commit_pending(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
        )?;

        assert!(log.all_events().is_empty());
        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Published);
        assert_eq!(EventLog::read_from_file(&events)?.len(), 1);
        Ok(())
    }

    #[test]
    fn commit_pending_rejects_a_different_package() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();
        log.record(PublishEvent {
            package: "other@1.0.0".to_string(),
            ..published_event()
        });

        let err = commit_pending(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
        )
        .expect_err("pending transition must match the buffered event package");

        assert!(err.to_string().contains("pending event package"));
        assert!(!events.exists());
        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Pending);
        Ok(())
    }

    #[test]
    fn commit_does_not_persist_state_when_event_append_fails() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let events = events_path(dir.path());
        std::fs::create_dir(&events)?;
        let mut state = state();
        let mut log = EventLog::new();

        let err = commit(
            &mut state,
            dir.path(),
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
            published_event(),
        )
        .expect_err("an events directory cannot be opened as a JSONL file");

        assert!(
            err.to_string()
                .contains("failed to persist package transition event")
        );
        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Pending);
        assert_eq!(log.all_events().len(), 1);
        Ok(())
    }

    #[test]
    fn commit_leaves_event_truth_when_state_projection_fails() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let state_dir = dir.path().join("state-file");
        File::create(&state_dir)?;
        let events = events_path(dir.path());
        let mut state = state();
        let mut log = EventLog::new();

        let err = commit(
            &mut state,
            &state_dir,
            &mut log,
            &events,
            "demo@1.0.0",
            PackageState::Published,
            published_event(),
        )
        .expect_err("a file cannot be used as the state directory");

        assert!(
            err.to_string()
                .contains("failed to persist package transition state")
        );
        assert_eq!(state.packages["demo@1.0.0"].state, PackageState::Pending);
        assert!(log.all_events().is_empty());
        let persisted = EventLog::read_from_file(&events)?;
        assert!(matches!(
            persisted
                .all_events()
                .first()
                .map(|event| &event.event_type),
            Some(EventType::PackagePublished { duration_ms: 7 })
        ));
        Ok(())
    }
}
