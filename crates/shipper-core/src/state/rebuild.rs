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
    ErrorClass, EventType, ExecutionState, PackageProgress, PackageState, PublishEvent,
    ReconciliationOutcome, Registry,
};

use crate::runtime::execution::pkg_key;

use super::{events, execution_state};

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

    for event in events {
        apply_event(event, &mut plan_id, &mut packages);
    }

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
        attempt_history: Vec::new(),
        packages,
    })
}

/// Rebuild and write `<state_dir>/state.json` from `<state_dir>/events.jsonl`.
pub fn rebuild_state_file_from_events(
    state_dir: &Path,
    options: StateRebuildOptions,
) -> Result<ExecutionState> {
    let events_path = events::events_path(state_dir);
    let state = rebuild_state_from_events(&events_path, options)?;
    execution_state::save_state(state_dir, &state)?;
    Ok(state)
}

fn apply_event(
    event: &PublishEvent,
    plan_id: &mut Option<String>,
    packages: &mut BTreeMap<String, PackageProgress>,
) {
    match &event.event_type {
        EventType::PlanCreated {
            plan_id: event_plan_id,
            ..
        } => {
            *plan_id = Some(event_plan_id.clone());
        }
        EventType::PackageStarted { name, version } => {
            let key = pkg_key(name, version);
            let progress = ensure_package(packages, &key, name, version, event.timestamp);
            progress.state = PackageState::Pending;
            progress.last_updated_at = event.timestamp;
        }
        EventType::PackageAttempted { attempt, .. } => {
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.attempts = progress.attempts.max(*attempt);
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PackagePublished { .. } => {
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
        EventType::PackageFailed { class, message } => {
            if let Some(progress) = ensure_event_package(packages, event, event.timestamp) {
                progress.state = match class {
                    ErrorClass::Ambiguous => PackageState::Ambiguous {
                        message: message.clone(),
                    },
                    _ => PackageState::Failed {
                        class: class.clone(),
                        message: message.clone(),
                    },
                };
                progress.last_updated_at = event.timestamp;
            }
        }
        EventType::PublishReconciled { outcome } => {
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
        _ => {}
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
                    },
                ),
                event(
                    2,
                    "demo@0.1.0",
                    EventType::PackageAttempted {
                        attempt: 3,
                        command: "cargo publish".to_string(),
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
                        package_count: 3,
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
