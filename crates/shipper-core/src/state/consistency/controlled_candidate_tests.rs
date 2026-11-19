use super::*;
use chrono::Utc;
use shipper_types::{PublishEvent, ReadinessMethod};

fn event(package: &str, event_type: EventType) -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        package: package.into(),
        event_type,
    }
}

fn absent(package: &str) -> PublishEvent {
    event(
        package,
        EventType::PublishReconciled {
            outcome: ReconciliationOutcome::NotPublished {
                attempts: 1,
                elapsed_ms: 0,
            },
        },
    )
}

fn retryable(package: &str) -> PublishEvent {
    event(
        package,
        EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "attempt failed".into(),
        },
    )
}

fn candidate() -> Vec<PublishEvent> {
    vec![
        event("all", EventType::ExecutionStarted),
        event("a@1.0.0", attempted()),
        absent("a@1.0.0"),
        retryable("a@1.0.0"),
    ]
}

fn attempted() -> EventType {
    EventType::PackageAttempted {
        attempt: 2,
        max_attempts: 2,
        command: "cargo publish -p a".into(),
    }
}

fn unknown() -> EventType {
    EventType::PublishReconciled {
        outcome: ReconciliationOutcome::StillUnknown {
            attempts: 1,
            elapsed_ms: 0,
            reason: "registry unavailable".into(),
        },
    }
}

fn detects(events: Vec<PublishEvent>) -> Result<bool> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("events.jsonl");
    let mut log = EventLog::new();
    for event in events {
        log.record(event);
    }
    log.write_to_file(&path)?;
    has_current_not_published_retryable_failure(&path)
}

#[test]
#[serial_test::serial]
fn candidate_requires_the_same_packages_ordered_current_attempt() -> Result<()> {
    assert!(detects(candidate())?);
    assert!(!detects(vec![absent("a@1.0.0")])?);
    assert!(!detects(vec![retryable("a@1.0.0")])?);
    assert!(!detects(vec![retryable("a@1.0.0"), absent("a@1.0.0")])?);
    assert!(!detects(vec![absent("a@1.0.0"), retryable("b@1.0.0")])?);
    assert!(!detects(vec![
        absent("a@1.0.0"),
        event("a@1.0.0", attempted()),
        retryable("a@1.0.0"),
    ])?);
    let mut events = candidate();
    events.push(event("a@1.0.0", attempted()));
    events.push(absent("a@1.0.0"));
    events.push(retryable("a@1.0.0"));
    assert!(detects(events)?);
    let mut events = candidate();
    events.extend(informational_starts());
    assert!(
        detects(events)?,
        "starts do not advance the retained attempt"
    );
    let mut events = vec![absent("a@1.0.0")];
    events.extend(informational_starts());
    events.push(retryable("a@1.0.0"));
    assert!(
        detects(events)?,
        "starts cannot split the same unresolved attempt"
    );
    Ok(())
}

fn informational_starts() -> Vec<PublishEvent> {
    vec![
        event("all", EventType::ExecutionStarted),
        event(
            "all",
            EventType::PlanCreated {
                plan_id: "same-plan".into(),
                package_count: 2,
            },
        ),
        event(
            "a@1.0.0",
            EventType::PackageStarted {
                name: "a".into(),
                version: "1.0.0".into(),
            },
        ),
        event(
            "b@1.0.0",
            EventType::PackageStarted {
                name: "b".into(),
                version: "1.0.0".into(),
            },
        ),
    ]
}

#[test]
#[serial_test::serial]
fn superseding_work_or_lifecycle_clears_a_historical_candidate() -> Result<()> {
    for superseding in [
        attempted(),
        EventType::PackageUploaded,
        EventType::PackagePublished { duration_ms: 0 },
        EventType::PackageSkipped {
            reason: "visible".into(),
        },
        EventType::PublishReconciling {
            method: ReadinessMethod::Api,
        },
        unknown(),
        EventType::PublishReconciled {
            outcome: ReconciliationOutcome::Published {
                attempts: 1,
                elapsed_ms: 0,
            },
        },
        EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "blocked".into(),
        },
        EventType::PackageFailed {
            class: ErrorClass::Ambiguous,
            message: "unknown".into(),
        },
        EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
    ] {
        let mut events = candidate();
        events.push(event("a@1.0.0", superseding.clone()));
        assert!(
            !detects(events)?,
            "historical candidate survived {superseding:?}"
        );
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn interleaved_success_keeps_the_candidate_but_other_recovery_owners_take_precedence() -> Result<()>
{
    for blocker in [
        attempted(),
        EventType::PackageUploaded,
        unknown(),
        EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "ordinary retry".into(),
        },
        EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "blocked".into(),
        },
        EventType::PackageFailed {
            class: ErrorClass::Ambiguous,
            message: "unknown".into(),
        },
    ] {
        for blocker_before_candidate in [false, true] {
            let mut events = candidate();
            let position = if blocker_before_candidate {
                1
            } else {
                events.len()
            };
            events.insert(position, event("b@1.0.0", blocker.clone()));
            assert!(
                !detects(events.clone())?,
                "mixed {blocker:?} gained controlled admission"
            );
            events.extend(informational_starts());
            assert!(
                !detects(events.clone())?,
                "start-only events erased {blocker:?} recovery ownership"
            );
            events.push(event(
                "b@1.0.0",
                EventType::PackagePublished { duration_ms: 0 },
            ));
            assert!(
                detects(events)?,
                "another package's success erased the candidate"
            );
        }
    }
    let mut events = candidate();
    events.insert(2, event("b@1.0.0", attempted()));
    events.insert(
        4,
        event("b@1.0.0", EventType::PackagePublished { duration_ms: 0 }),
    );
    assert!(detects(events)?);
    let mut events = candidate();
    events.extend([absent("b@1.0.0"), retryable("b@1.0.0")]);
    assert!(detects(events)?);
    Ok(())
}
