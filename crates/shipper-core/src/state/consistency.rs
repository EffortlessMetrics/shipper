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

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};

use shipper_types::{EventType, ExecutionState, PackageState, StateEventDrift};

use super::events::EventLog;

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

    // Package labels that have a `PackagePublished` event in events.jsonl.
    // Labels are the `package` field of the event (format: `name@version`).
    let events_published: BTreeSet<String> = log
        .all_events()
        .iter()
        .filter(|e| matches!(e.event_type, EventType::PackagePublished { .. }))
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

/// Render a human-readable summary of a drift report. Used by the Reporter
/// to surface the finding loudly at end of run.
pub fn format_drift_summary(drift: &StateEventDrift) -> String {
    if drift.is_consistent() {
        return "events.jsonl and state.json are consistent".to_string();
    }

    let mut lines = Vec::new();
    lines.push("state/event drift detected (events.jsonl is authoritative):".to_string());
    if !drift.in_events_only.is_empty() {
        lines.push(format!(
            "  published in events.jsonl but NOT in state.json ({}): {}",
            drift.in_events_only.len(),
            drift.in_events_only.join(", ")
        ));
    }
    if !drift.in_state_only.is_empty() {
        lines.push(format!(
            "  marked published in state.json but NO event ({}): {}",
            drift.in_state_only.len(),
            drift.in_state_only.join(", ")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use shipper_types::{ExecutionState, PackageProgress, PublishEvent, Registry};
    use tempfile::tempdir;

    use super::*;
    use crate::state::events::{EVENTS_FILE, EventLog};

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
    fn detects_in_events_only() {
        // events says published, state says pending → resume would duplicate
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
        // state says published but no event recorded it → event log bypassed
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
    fn empty_state_and_empty_events_are_consistent() {
        let td = tempdir().expect("tempdir");
        // No events file written at all — read_from_file treats missing as empty.
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
}
