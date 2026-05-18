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

use anyhow::{Context, Result, bail};

use shipper_types::{
    EventType, ExecutionState, PackageState, Receipt, ReconciliationReport, StateEventDrift,
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
    verify_plan_ids(receipt, &rebuilt_state, &mut findings);
    verify_state_matches_events(
        state,
        &rebuilt_state,
        &trusted_resume_terminal_skips,
        &mut findings,
    );
    verify_receipt_matches_state(state, receipt, &mut findings);
    verify_reconciliation_evidence(&event_log, receipt, reconciliation_report, &mut findings);

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

fn verify_reconciliation_evidence(
    event_log: &EventLog,
    receipt: &Receipt,
    reconciliation_report: Option<&ReconciliationReport>,
    findings: &mut Vec<String>,
) {
    let reconciled_packages: BTreeSet<String> = event_log
        .all_events()
        .iter()
        .filter(|event| matches!(event.event_type, EventType::PublishReconciled { .. }))
        .map(|event| event.package.clone())
        .collect();

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

    if report.plan_id != receipt.plan_id {
        findings.push(format!(
            "reconciliation plan_id {} does not match receipt plan_id {}",
            report.plan_id, receipt.plan_id
        ));
    }

    let report_packages: BTreeSet<String> = report
        .records
        .iter()
        .map(|record| record.package.clone())
        .collect();

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
}

fn state_name(state: &PackageState) -> &'static str {
    match state {
        PackageState::Pending => "pending",
        PackageState::Uploaded => "uploaded",
        PackageState::Published => "published",
        PackageState::Skipped { .. } => "skipped",
        PackageState::Failed { .. } => "failed",
        PackageState::Ambiguous { .. } => "ambiguous",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use shipper_types::{
        EnvironmentFingerprint, PackageEvidence, PackageProgress, PackageReceipt, PublishEvent,
        ReconciliationEvidenceKind, ReconciliationEvidenceSource, ReconciliationOperatorAction,
        ReconciliationRecord, ReconciliationReport, ReconciliationTrigger, Registry,
    };
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
