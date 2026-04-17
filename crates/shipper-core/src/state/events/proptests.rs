//! Property-based tests for `crate::state::events`.
//!
//! Absorbed from the former `shipper-events` crate's inline `proptests` module.

use super::*;
use chrono::Utc;
use proptest::prelude::*;
use shipper_types::{ErrorClass, EventType, ExecutionResult, Finishability, ReadinessMethod};
use tempfile::tempdir;

fn arb_error_class() -> impl Strategy<Value = ErrorClass> {
    prop_oneof![
        Just(ErrorClass::Retryable),
        Just(ErrorClass::Permanent),
        Just(ErrorClass::Ambiguous),
    ]
}

fn arb_execution_result() -> impl Strategy<Value = ExecutionResult> {
    prop_oneof![
        Just(ExecutionResult::Success),
        Just(ExecutionResult::PartialFailure),
        Just(ExecutionResult::CompleteFailure),
    ]
}

fn arb_readiness_method() -> impl Strategy<Value = ReadinessMethod> {
    prop_oneof![
        Just(ReadinessMethod::Api),
        Just(ReadinessMethod::Index),
        Just(ReadinessMethod::Both),
    ]
}

fn arb_finishability() -> impl Strategy<Value = Finishability> {
    prop_oneof![
        Just(Finishability::Proven),
        Just(Finishability::NotProven),
        Just(Finishability::Failed),
    ]
}

fn arb_event_type() -> impl Strategy<Value = EventType> {
    prop_oneof![
        (".*", 0..100usize).prop_map(|(id, count)| EventType::PlanCreated {
            plan_id: id,
            package_count: count,
        }),
        Just(EventType::ExecutionStarted),
        arb_execution_result().prop_map(|result| EventType::ExecutionFinished { result }),
        (".*", ".*").prop_map(|(name, version)| EventType::PackageStarted { name, version }),
        (1..100u32, ".*")
            .prop_map(|(attempt, command)| EventType::PackageAttempted { attempt, command }),
        (".*", ".*").prop_map(|(stdout_tail, stderr_tail)| EventType::PackageOutput {
            stdout_tail,
            stderr_tail,
        }),
        (0..u64::MAX).prop_map(|d| EventType::PackagePublished { duration_ms: d }),
        (arb_error_class(), ".*")
            .prop_map(|(class, message)| EventType::PackageFailed { class, message }),
        ".*".prop_map(|reason| EventType::PackageSkipped { reason }),
        arb_readiness_method().prop_map(|method| EventType::ReadinessStarted { method }),
        (1..100u32, any::<bool>())
            .prop_map(|(attempt, visible)| EventType::ReadinessPoll { attempt, visible }),
        (0..u64::MAX, 1..100u32).prop_map(|(d, a)| EventType::ReadinessComplete {
            duration_ms: d,
            attempts: a,
        }),
        (0..u64::MAX).prop_map(|d| EventType::ReadinessTimeout { max_wait_ms: d }),
        Just(EventType::PreflightStarted),
        (any::<bool>(), ".*").prop_map(|(passed, output)| {
            EventType::PreflightWorkspaceVerify { passed, output }
        }),
        ".*".prop_map(|crate_name| EventType::PreflightNewCrateDetected { crate_name }),
        (".*", any::<bool>()).prop_map(|(crate_name, verified)| {
            EventType::PreflightOwnershipCheck {
                crate_name,
                verified,
            }
        }),
        arb_finishability()
            .prop_map(|finishability| EventType::PreflightComplete { finishability }),
        (".*", ".*").prop_map(|(crate_name, version)| EventType::IndexReadinessStarted {
            crate_name,
            version,
        }),
        (".*", ".*", any::<bool>()).prop_map(|(crate_name, version, found)| {
            EventType::IndexReadinessCheck {
                crate_name,
                version,
                found,
            }
        }),
        (".*", ".*", any::<bool>()).prop_map(|(crate_name, version, visible)| {
            EventType::IndexReadinessComplete {
                crate_name,
                version,
                visible,
            }
        }),
    ]
}

fn arb_publish_event() -> impl Strategy<Value = PublishEvent> {
    (arb_event_type(), ".*").prop_map(|(event_type, package)| PublishEvent {
        timestamp: Utc::now(),
        event_type,
        package,
    })
}

proptest! {
    #[test]
    fn any_event_serializes_and_deserializes(event in arb_publish_event()) {
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(&parsed.package, &event.package);
    }

    #[test]
    fn any_event_produces_single_json_line(event in arb_publish_event()) {
        let json = serde_json::to_string(&event).expect("serialize");
        // serde_json::to_string should never produce embedded newlines
        prop_assert!(!json.contains('\n'), "JSON contains newline: {}", json);
    }

    #[test]
    fn roundtrip_via_file_preserves_count(events in proptest::collection::vec(arb_publish_event(), 0..20)) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }
        log.write_to_file(&path).expect("write");

        let loaded = EventLog::read_from_file(&path).expect("read");
        prop_assert_eq!(loaded.len(), events.len());
    }

    #[test]
    fn roundtrip_via_file_preserves_packages(events in proptest::collection::vec(arb_publish_event(), 1..10)) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }
        log.write_to_file(&path).expect("write");

        let loaded = EventLog::read_from_file(&path).expect("read");
        for (orig, read) in events.iter().zip(loaded.all_events().iter()) {
            prop_assert_eq!(&orig.package, &read.package);
            prop_assert_eq!(orig.timestamp, read.timestamp);
        }
    }

    #[test]
    fn package_filter_never_returns_wrong_package(
        events in proptest::collection::vec(arb_publish_event(), 1..15),
        filter_pkg in ".*",
    ) {
        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }
        let filtered = log.events_for_package(&filter_pkg);
        for e in filtered {
            prop_assert_eq!(&e.package, &filter_pkg);
        }
    }

    #[test]
    fn len_matches_all_events_len(events in proptest::collection::vec(arb_publish_event(), 0..20)) {
        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }
        prop_assert_eq!(log.len(), log.all_events().len());
        prop_assert_eq!(log.is_empty(), events.is_empty());
    }

    #[test]
    fn multiple_appends_preserve_global_order(
        batches in proptest::collection::vec(
            proptest::collection::vec(arb_publish_event(), 1..5),
            1..5,
        ),
    ) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let mut all_packages: Vec<String> = Vec::new();
        for batch in &batches {
            let mut log = EventLog::new();
            for e in batch {
                log.record(e.clone());
                all_packages.push(e.package.clone());
            }
            log.write_to_file(&path).expect("write");
        }

        let loaded = EventLog::read_from_file(&path).expect("read");
        prop_assert_eq!(loaded.len(), all_packages.len());
        for (i, event) in loaded.all_events().iter().enumerate() {
            prop_assert_eq!(&event.package, &all_packages[i]);
        }
    }

    #[test]
    fn timestamps_preserved_monotonically_after_roundtrip(
        n in 2..20usize,
    ) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let mut log = EventLog::new();
        let mut timestamps = Vec::new();
        for i in 0..n {
            let ts = Utc::now();
            timestamps.push(ts);
            log.record(PublishEvent {
                timestamp: ts,
                event_type: EventType::PackagePublished { duration_ms: i as u64 },
                package: format!("pkg-{i}@1.0.0"),
            });
        }
        log.write_to_file(&path).expect("write");

        let loaded = EventLog::read_from_file(&path).expect("read");
        let loaded_events = loaded.all_events();
        for i in 0..n {
            prop_assert_eq!(loaded_events[i].timestamp, timestamps[i]);
        }
        // Verify monotonicity (non-decreasing)
        for i in 1..loaded_events.len() {
            prop_assert!(
                loaded_events[i].timestamp >= loaded_events[i - 1].timestamp,
                "timestamps not monotonic at index {}", i
            );
        }
    }

    #[test]
    fn filter_returns_all_matching_events(
        events in proptest::collection::vec(arb_publish_event(), 1..20),
    ) {
        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }

        // For each unique package, filter count should match manual count
        let packages: std::collections::HashSet<&str> =
            events.iter().map(|e| e.package.as_str()).collect();
        for pkg in packages {
            let expected = events.iter().filter(|e| e.package == pkg).count();
            let filtered = log.events_for_package(pkg);
            prop_assert_eq!(filtered.len(), expected);
        }
    }

    #[test]
    fn clear_then_rerecord_has_only_new_events(
        old_events in proptest::collection::vec(arb_publish_event(), 1..10),
        new_events in proptest::collection::vec(arb_publish_event(), 1..10),
    ) {
        let mut log = EventLog::new();
        for e in &old_events {
            log.record(e.clone());
        }
        log.clear();
        for e in &new_events {
            log.record(e.clone());
        }
        prop_assert_eq!(log.len(), new_events.len());
        for (i, e) in log.all_events().iter().enumerate() {
            prop_assert_eq!(&e.package, &new_events[i].package);
        }
    }

    #[test]
    fn jsonl_lines_match_event_count_on_disk(
        events in proptest::collection::vec(arb_publish_event(), 0..20),
    ) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }
        log.write_to_file(&path).expect("write");

        let content = std::fs::read_to_string(&path).expect("read");
        let line_count = if content.is_empty() { 0 } else { content.lines().count() };
        prop_assert_eq!(line_count, events.len());
    }

    #[test]
    fn roundtrip_json_preserves_all_fields(event in arb_publish_event()) {
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");

        // Re-serialize and compare JSON to ensure full fidelity
        let json2 = serde_json::to_string(&parsed).expect("re-serialize");
        prop_assert_eq!(&json, &json2, "JSON roundtrip mismatch");
    }

    #[test]
    fn roundtrip_via_file_preserves_json_fidelity(events in proptest::collection::vec(arb_publish_event(), 1..10)) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let mut log = EventLog::new();
        let orig_jsons: Vec<String> = events
            .iter()
            .map(|e| {
                log.record(e.clone());
                serde_json::to_string(e).expect("serialize")
            })
            .collect();
        log.write_to_file(&path).expect("write");

        let loaded = EventLog::read_from_file(&path).expect("read");
        for (orig_json, loaded_event) in orig_jsons.iter().zip(loaded.all_events().iter()) {
            let loaded_json = serde_json::to_string(loaded_event).expect("re-serialize");
            prop_assert_eq!(orig_json, &loaded_json, "File roundtrip JSON mismatch");
        }
    }

    #[test]
    fn any_event_json_has_required_top_level_keys(event in arb_publish_event()) {
        let json = serde_json::to_string(&event).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");
        let obj = value.as_object().expect("should be JSON object");
        prop_assert!(obj.contains_key("timestamp"), "missing timestamp key");
        prop_assert!(obj.contains_key("event_type"), "missing event_type key");
        prop_assert!(obj.contains_key("package"), "missing package key");
        let et = obj.get("event_type").unwrap().as_object().expect("event_type should be object");
        prop_assert!(et.contains_key("type"), "event_type missing type discriminator");
    }

    #[test]
    fn filter_correctness_after_file_roundtrip(
        events in proptest::collection::vec(arb_publish_event(), 1..15),
    ) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }
        log.write_to_file(&path).expect("write");

        let loaded = EventLog::read_from_file(&path).expect("read");
        let packages: std::collections::HashSet<&str> =
            events.iter().map(|e| e.package.as_str()).collect();
        for pkg in packages {
            let expected = events.iter().filter(|e| e.package == pkg).count();
            let filtered = loaded.events_for_package(pkg);
            prop_assert_eq!(filtered.len(), expected, "filter mismatch for {}", pkg);
        }
    }

    /// Double-roundtrip: write→read→write→read produces identical events.
    #[test]
    fn double_roundtrip_is_idempotent(events in proptest::collection::vec(arb_publish_event(), 1..10)) {
        let td = tempdir().expect("tempdir");
        let path1 = td.path().join("events1.jsonl");
        let path2 = td.path().join("events2.jsonl");

        let mut log1 = EventLog::new();
        for e in &events {
            log1.record(e.clone());
        }
        log1.write_to_file(&path1).expect("write1");

        let loaded1 = EventLog::read_from_file(&path1).expect("read1");
        loaded1.write_to_file(&path2).expect("write2");

        let loaded2 = EventLog::read_from_file(&path2).expect("read2");
        prop_assert_eq!(loaded1.len(), loaded2.len());
        for (a, b) in loaded1.all_events().iter().zip(loaded2.all_events().iter()) {
            let ja = serde_json::to_string(a).unwrap();
            let jb = serde_json::to_string(b).unwrap();
            prop_assert_eq!(ja, jb, "double-roundtrip mismatch");
        }
    }

    /// Filter partition: sum of per-package filtered counts equals total event count.
    #[test]
    fn filter_partition_covers_all_events(events in proptest::collection::vec(arb_publish_event(), 0..20)) {
        let mut log = EventLog::new();
        for e in &events {
            log.record(e.clone());
        }

        let packages: std::collections::HashSet<&str> =
            events.iter().map(|e| e.package.as_str()).collect();
        let total_from_filters: usize = packages
            .iter()
            .map(|pkg| log.events_for_package(pkg).len())
            .sum();
        prop_assert_eq!(total_from_filters, events.len(),
            "sum of per-package filtered counts should equal total");
    }

    /// Timestamp ordering: events inserted with non-decreasing timestamps
    /// retain that ordering after file roundtrip.
    #[test]
    fn sorted_timestamps_preserved_after_roundtrip(n in 1usize..15) {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("events.jsonl");

        let base = Utc::now();
        let mut log = EventLog::new();
        for i in 0..n {
            log.record(shipper_types::PublishEvent {
                timestamp: base + chrono::Duration::seconds(i as i64),
                event_type: shipper_types::EventType::PackagePublished { duration_ms: i as u64 },
                package: format!("pkg-{i}"),
            });
        }
        log.write_to_file(&path).expect("write");

        let loaded = EventLog::read_from_file(&path).expect("read");
        let events = loaded.all_events();
        for i in 1..events.len() {
            prop_assert!(
                events[i].timestamp >= events[i - 1].timestamp,
                "timestamp ordering broken at index {i}"
            );
        }
    }
}
