//! Persist reconciliation evidence derived from the authoritative event log.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use shipper_types::{
    ErrorClass, EventType, ReadinessMethod, ReconciliationEvidenceKind,
    ReconciliationEvidenceSource, ReconciliationOperatorAction, ReconciliationRecord,
    ReconciliationReport, ReconciliationTrigger, Registry,
};

use super::events::EventLog;
use super::execution_state;

pub const RECONCILIATION_SCHEMA_VERSION: &str = "shipper.reconciliation.v1";

pub fn write_report_from_events(
    state_dir: &Path,
    plan_id: &str,
    registry: &Registry,
    events_path: &Path,
) -> Result<Option<ReconciliationReport>> {
    let log = EventLog::read_from_file(events_path)
        .with_context(|| format!("failed to read event log from {}", events_path.display()))?;
    let records = records_from_event_log(&log);
    let report_path = execution_state::reconciliation_path(state_dir);

    if records.is_empty() {
        if report_path.exists() {
            std::fs::remove_file(&report_path).with_context(|| {
                format!(
                    "failed to remove stale reconciliation report {}",
                    report_path.display()
                )
            })?;
        }
        return Ok(None);
    }

    let evidence_sources = vec![ReconciliationEvidenceSource {
        kind: ReconciliationEvidenceKind::EventLog,
        path: events_path.display().to_string(),
    }];

    let report = ReconciliationReport {
        schema_version: RECONCILIATION_SCHEMA_VERSION.to_string(),
        plan_id: plan_id.to_string(),
        registry: registry.clone(),
        generated_at: Utc::now(),
        evidence_sources,
        records,
    };

    execution_state::write_reconciliation_report(state_dir, &report)?;
    Ok(Some(report))
}

fn records_from_event_log(log: &EventLog) -> Vec<ReconciliationRecord> {
    let mut methods: BTreeMap<String, ReadinessMethod> = BTreeMap::new();
    let mut ambiguous_cargo_failures = BTreeSet::new();
    let mut records = Vec::new();

    for event in log.all_events() {
        match &event.event_type {
            EventType::PackageFailed {
                class: ErrorClass::Ambiguous,
                ..
            } => {
                ambiguous_cargo_failures.insert(event.package.clone());
            }
            EventType::PublishReconciling { method } => {
                methods.insert(event.package.clone(), *method);
            }
            EventType::PublishReconciled { outcome } => {
                let trigger = if ambiguous_cargo_failures.contains(&event.package) {
                    ReconciliationTrigger::CargoAmbiguousExit
                } else {
                    ReconciliationTrigger::ResumeAmbiguousState
                };
                ambiguous_cargo_failures.remove(&event.package);
                let cargo_exit_class = match trigger {
                    ReconciliationTrigger::CargoAmbiguousExit => Some(ErrorClass::Ambiguous),
                    ReconciliationTrigger::ResumeAmbiguousState => None,
                };
                let operator_action = match outcome {
                    shipper_types::ReconciliationOutcome::Published { .. } => {
                        ReconciliationOperatorAction::MarkPublishedContinue
                    }
                    shipper_types::ReconciliationOutcome::NotPublished { .. } => {
                        ReconciliationOperatorAction::RetryAllowed
                    }
                    shipper_types::ReconciliationOutcome::StillUnknown { .. } => {
                        ReconciliationOperatorAction::OperatorActionRequired
                    }
                };
                let (name, version) = split_package_label(&event.package);
                records.push(ReconciliationRecord {
                    package: event.package.clone(),
                    name,
                    version,
                    trigger,
                    method: methods.get(&event.package).copied(),
                    cargo_exit_class,
                    outcome: outcome.clone(),
                    operator_action,
                });
            }
            _ => {}
        }
    }

    records
}

fn split_package_label(package: &str) -> (String, String) {
    if let Some((name, version)) = package.rsplit_once('@') {
        return (name.to_string(), version.to_string());
    }
    (package.to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use shipper_types::{
        ErrorClass, EventType, PublishEvent, ReadinessMethod, ReconciliationOutcome,
    };
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn writes_reconciliation_report_from_reconciled_events() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let events_path = super::super::events::events_path(&state_dir);
        let mut log = EventLog::new();
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageFailed {
                class: ErrorClass::Ambiguous,
                message: "cargo exited before registry truth was known".to_string(),
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciling {
                method: ReadinessMethod::Both,
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciled {
                outcome: ReconciliationOutcome::Published {
                    attempts: 2,
                    elapsed_ms: 1500,
                },
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.write_to_file(&events_path).expect("write events");

        let report =
            write_report_from_events(&state_dir, "plan-123", &Registry::crates_io(), &events_path)
                .expect("write report")
                .expect("report");

        assert_eq!(report.schema_version, RECONCILIATION_SCHEMA_VERSION);
        assert_eq!(report.plan_id, "plan-123");
        assert_eq!(report.evidence_sources.len(), 1);
        assert_eq!(
            report.evidence_sources[0].kind,
            ReconciliationEvidenceKind::EventLog
        );
        assert_eq!(report.records.len(), 1);
        let record = &report.records[0];
        assert_eq!(record.package, "shipper-core@0.4.0");
        assert_eq!(record.name, "shipper-core");
        assert_eq!(record.version, "0.4.0");
        assert_eq!(record.trigger, ReconciliationTrigger::CargoAmbiguousExit);
        assert_eq!(record.method, Some(ReadinessMethod::Both));
        assert_eq!(record.cargo_exit_class, Some(ErrorClass::Ambiguous));
        assert_eq!(
            record.operator_action,
            ReconciliationOperatorAction::MarkPublishedContinue
        );
        assert!(execution_state::reconciliation_path(&state_dir).exists());
    }

    #[test]
    fn removes_stale_report_when_current_events_have_no_reconciliation() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        std::fs::create_dir_all(&state_dir).expect("mkdir");
        let report_path = execution_state::reconciliation_path(&state_dir);
        std::fs::write(&report_path, "{}").expect("write stale");
        let events_path = super::super::events::events_path(&state_dir);

        let report =
            write_report_from_events(&state_dir, "plan-123", &Registry::crates_io(), &events_path)
                .expect("write report");

        assert!(report.is_none());
        assert!(!report_path.exists());
    }

    #[test]
    fn classifies_later_resume_reconciliation_after_prior_cargo_ambiguity() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().join(".shipper");
        let events_path = super::super::events::events_path(&state_dir);
        let mut log = EventLog::new();
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageFailed {
                class: ErrorClass::Ambiguous,
                message: "cargo exited before registry truth was known".to_string(),
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciling {
                method: ReadinessMethod::Both,
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciled {
                outcome: ReconciliationOutcome::StillUnknown {
                    attempts: 1,
                    elapsed_ms: 20,
                    reason: "registry unavailable".to_string(),
                },
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciling {
                method: ReadinessMethod::Both,
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.record(PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PublishReconciled {
                outcome: ReconciliationOutcome::Published {
                    attempts: 1,
                    elapsed_ms: 15,
                },
            },
            package: "shipper-core@0.4.0".to_string(),
        });
        log.write_to_file(&events_path).expect("write events");

        let report =
            write_report_from_events(&state_dir, "plan-123", &Registry::crates_io(), &events_path)
                .expect("write report")
                .expect("report");

        assert_eq!(report.records.len(), 2);
        assert_eq!(
            report.records[0].trigger,
            ReconciliationTrigger::CargoAmbiguousExit
        );
        assert_eq!(
            report.records[1].trigger,
            ReconciliationTrigger::ResumeAmbiguousState
        );
        assert_eq!(report.records[1].cargo_exit_class, None);
    }
}
