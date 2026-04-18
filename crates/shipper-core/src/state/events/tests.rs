//! Unit tests for `crate::state::events`.
//!
//! Absorbed from the former `shipper-events` crate's inline `tests` module.

use super::*;
use chrono::{DateTime, Utc};
use shipper_types::{ErrorClass, EventType, ExecutionResult, Finishability, ReadinessMethod};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn fixed_time() -> DateTime<Utc> {
    "2025-01-15T12:00:00Z".parse::<DateTime<Utc>>().unwrap()
}

fn sample_event(package: &str) -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::PackageStarted {
            name: package.split('@').next().unwrap_or(package).to_string(),
            version: package.split('@').nth(1).unwrap_or("1.0.0").to_string(),
        },
        package: package.to_string(),
    }
}

fn fixed_event(event_type: EventType, package: &str) -> PublishEvent {
    PublishEvent {
        timestamp: fixed_time(),
        event_type,
        package: package.to_string(),
    }
}

fn make_event(event_type: EventType, package: &str) -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        event_type,
        package: package.to_string(),
    }
}

// -- Basic EventLog operations --

#[test]
fn new_event_log_is_empty() {
    let log = EventLog::new();
    assert!(log.is_empty());
    assert_eq!(log.len(), 0);
}

#[test]
fn default_event_log_is_empty() {
    let log = EventLog::default();
    assert!(log.is_empty());
    assert_eq!(log.len(), 0);
    assert_eq!(log.all_events().len(), 0);
}

#[test]
fn record_adds_event_to_log() {
    let mut log = EventLog::new();
    let event = sample_event("test@1.0.0");
    log.record(event);
    assert_eq!(log.len(), 1);
    assert!(!log.is_empty());
}

#[test]
fn record_multiple_events_preserves_order() {
    let mut log = EventLog::new();
    log.record(sample_event("a@1.0.0"));
    log.record(sample_event("b@2.0.0"));
    log.record(sample_event("c@3.0.0"));
    assert_eq!(log.len(), 3);

    let events = log.all_events();
    assert_eq!(events[0].package, "a@1.0.0");
    assert_eq!(events[1].package, "b@2.0.0");
    assert_eq!(events[2].package, "c@3.0.0");
}

#[test]
fn all_events_returns_slice_of_recorded_events() {
    let mut log = EventLog::new();
    log.record(sample_event("x@1.0.0"));
    let events = log.all_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].package, "x@1.0.0");
}

// -- Filtering --

#[test]
fn events_for_package_filters_correctly() {
    let mut log = EventLog::new();
    log.record(sample_event("pkg1@1.0.0"));
    log.record(sample_event("pkg2@1.0.0"));
    log.record(sample_event("pkg1@2.0.0"));

    let pkg1_events = log.events_for_package("pkg1@1.0.0");
    assert_eq!(pkg1_events.len(), 1);

    let pkg2_events = log.events_for_package("pkg2@1.0.0");
    assert_eq!(pkg2_events.len(), 1);
}

#[test]
fn events_for_package_returns_empty_when_no_match() {
    let mut log = EventLog::new();
    log.record(sample_event("foo@1.0.0"));
    let results = log.events_for_package("bar@1.0.0");
    assert!(results.is_empty());
}

#[test]
fn events_for_package_returns_empty_on_empty_log() {
    let log = EventLog::new();
    let results = log.events_for_package("anything");
    assert!(results.is_empty());
}

#[test]
fn events_for_package_matching_is_exact() {
    let mut log = EventLog::new();
    log.record(sample_event("pkg@1.0.0"));
    log.record(sample_event("pkg@1.0.0-beta"));
    log.record(sample_event("my-pkg@1.0.0"));

    assert_eq!(log.events_for_package("pkg@1.0.0").len(), 1);
    assert_eq!(log.events_for_package("pkg@1.0.0-beta").len(), 1);
    assert_eq!(log.events_for_package("pkg").len(), 0);
}

// -- Clear --

#[test]
fn clear_removes_all_events() {
    let mut log = EventLog::new();
    log.record(sample_event("test@1.0.0"));
    log.record(sample_event("test@2.0.0"));
    assert_eq!(log.len(), 2);

    log.clear();
    assert!(log.is_empty());
    assert_eq!(log.len(), 0);
    assert!(log.all_events().is_empty());
}

#[test]
fn clear_on_empty_log_is_noop() {
    let mut log = EventLog::new();
    log.clear();
    assert!(log.is_empty());
}

// -- File I/O --

#[test]
fn write_to_file_creates_jsonl_format() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(sample_event("test@1.0.0"));

    log.write_to_file(&path).expect("write");

    let content = fs::read_to_string(&path).expect("read");
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 1);

    // Verify it's valid JSON
    let _: PublishEvent = serde_json::from_str(lines[0]).expect("parse");
}

#[test]
fn write_to_file_appends_to_existing_file() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log1 = EventLog::new();
    log1.record(sample_event("test@1.0.0"));
    log1.write_to_file(&path).expect("write first");

    let mut log2 = EventLog::new();
    log2.record(sample_event("test@2.0.0"));
    log2.write_to_file(&path).expect("write second");

    let content = fs::read_to_string(&path).expect("read");
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn write_to_file_creates_parent_directories() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("nested").join("deep").join("events.jsonl");

    let mut log = EventLog::new();
    log.record(sample_event("test@1.0.0"));
    log.write_to_file(&path).expect("write to nested path");

    assert!(path.exists());
    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);
}

#[test]
fn write_empty_log_creates_empty_file() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let log = EventLog::new();
    log.write_to_file(&path).expect("write");

    let content = fs::read_to_string(&path).expect("read");
    assert!(content.is_empty());
}

#[test]
fn read_from_file_loads_all_events() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(sample_event("test@1.0.0"));
    log.record(sample_event("test@2.0.0"));
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 2);
}

#[test]
fn read_from_file_returns_empty_log_when_missing() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("nonexistent.jsonl");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert!(loaded.is_empty());
}

#[test]
fn read_from_file_errors_on_invalid_json() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("bad.jsonl");
    fs::write(&path, "not valid json\n").expect("write bad file");

    let result = EventLog::read_from_file(&path);
    assert!(result.is_err());
}

#[test]
fn read_from_file_errors_on_partial_corruption() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    // Write one valid event, then corrupt data
    let mut log = EventLog::new();
    log.record(sample_event("ok@1.0.0"));
    log.write_to_file(&path).expect("write");

    // Append invalid line
    let mut file = OpenOptions::new().append(true).open(&path).expect("open");
    writeln!(file, "{{bad json}}").expect("write bad line");

    let result = EventLog::read_from_file(&path);
    assert!(result.is_err());
}

// -- Roundtrip serialization --

#[test]
fn roundtrip_write_then_read_preserves_events() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(make_event(
        EventType::PlanCreated {
            plan_id: "plan-abc".to_string(),
            package_count: 5,
        },
        "all",
    ));
    log.record(make_event(EventType::ExecutionStarted, "all"));
    log.record(make_event(
        EventType::PackageStarted {
            name: "my-crate".to_string(),
            version: "0.1.0".to_string(),
        },
        "my-crate@0.1.0",
    ));
    log.record(make_event(
        EventType::PackagePublished { duration_ms: 4200 },
        "my-crate@0.1.0",
    ));
    log.record(make_event(
        EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        "all",
    ));

    log.write_to_file(&path).expect("write");
    let loaded = EventLog::read_from_file(&path).expect("read");

    assert_eq!(loaded.len(), log.len());
    for (orig, read) in log.all_events().iter().zip(loaded.all_events().iter()) {
        assert_eq!(orig.package, read.package);
        assert_eq!(orig.timestamp, read.timestamp);
    }
}

#[test]
fn roundtrip_preserves_timestamp_precision() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let ts = Utc::now();
    let event = PublishEvent {
        timestamp: ts,
        event_type: EventType::ExecutionStarted,
        package: "ts-test".to_string(),
    };

    let mut log = EventLog::new();
    log.record(event);
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.all_events()[0].timestamp, ts);
}

// -- JSONL format validation --

#[test]
fn each_line_is_independent_valid_json() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    for i in 0..5 {
        log.record(sample_event(&format!("pkg{i}@1.0.0")));
    }
    log.write_to_file(&path).expect("write");

    let content = fs::read_to_string(&path).expect("read");
    for (i, line) in content.lines().enumerate() {
        let parsed: Result<PublishEvent, _> = serde_json::from_str(line);
        assert!(parsed.is_ok(), "line {i} is not valid JSON: {line}");
    }
}

#[test]
fn jsonl_lines_contain_no_embedded_newlines() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    // Event with newlines in payload strings
    log.record(make_event(
        EventType::PackageOutput {
            stdout_tail: "line1\nline2\nline3".to_string(),
            stderr_tail: "err\nmore".to_string(),
        },
        "test@1.0.0",
    ));
    log.write_to_file(&path).expect("write");

    let content = fs::read_to_string(&path).expect("read");
    let lines: Vec<&str> = content.lines().collect();
    // Should be exactly 1 line despite embedded newlines in data
    assert_eq!(lines.len(), 1);
    let _: PublishEvent = serde_json::from_str(lines[0]).expect("valid JSON");
}

#[test]
fn jsonl_uses_tagged_enum_format() {
    let event = make_event(
        EventType::PackageStarted {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
        },
        "foo@1.0.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse");

    // EventType uses #[serde(tag = "type", rename_all = "snake_case")]
    let event_type_obj = value.get("event_type").expect("event_type field exists");
    let type_tag = event_type_obj
        .get("type")
        .expect("type tag exists")
        .as_str()
        .expect("type is string");
    assert_eq!(type_tag, "package_started");
}

// -- All EventType variant serialization roundtrips --

#[test]
fn event_types_serialize_correctly() {
    let events = vec![
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PlanCreated {
                plan_id: "plan-1".to_string(),
                package_count: 3,
            },
            package: "all".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ExecutionStarted,
            package: "all".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ExecutionFinished {
                result: ExecutionResult::Success,
            },
            package: "all".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageStarted {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageAttempted {
                attempt: 1,
                command: "cargo publish".to_string(),
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageOutput {
                stdout_tail: "some output".to_string(),
                stderr_tail: "some error".to_string(),
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackagePublished { duration_ms: 1000 },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageFailed {
                class: ErrorClass::Permanent,
                message: "failed".to_string(),
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageSkipped {
                reason: "already published".to_string(),
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessStarted {
                method: ReadinessMethod::Api,
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessPoll {
                attempt: 1,
                visible: false,
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessComplete {
                duration_ms: 5000,
                attempts: 3,
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::ReadinessTimeout {
                max_wait_ms: 300000,
            },
            package: "test@1.0.0".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightStarted,
            package: "all".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightWorkspaceVerify {
                passed: true,
                output: "dry-run output".to_string(),
            },
            package: "all".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightNewCrateDetected {
                crate_name: "newcrate".to_string(),
            },
            package: "all".to_string(),
        },
        PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PreflightComplete {
                finishability: Finishability::Proven,
            },
            package: "all".to_string(),
        },
    ];

    for event in events {
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.package, event.package);
    }
}

#[test]
fn all_execution_result_variants_roundtrip() {
    for result in [
        ExecutionResult::Success,
        ExecutionResult::PartialFailure,
        ExecutionResult::CompleteFailure,
    ] {
        let event = make_event(
            EventType::ExecutionFinished {
                result: result.clone(),
            },
            "all",
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let _: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    }
}

#[test]
fn all_error_class_variants_roundtrip() {
    for class in [
        ErrorClass::Retryable,
        ErrorClass::Permanent,
        ErrorClass::Ambiguous,
    ] {
        let event = make_event(
            EventType::PackageFailed {
                class: class.clone(),
                message: "test".to_string(),
            },
            "test@1.0.0",
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let _: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    }
}

#[test]
fn all_readiness_method_variants_roundtrip() {
    for method in [
        ReadinessMethod::Api,
        ReadinessMethod::Index,
        ReadinessMethod::Both,
    ] {
        let event = make_event(EventType::ReadinessStarted { method }, "test@1.0.0");
        let json = serde_json::to_string(&event).expect("serialize");
        let _: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    }
}

#[test]
fn all_finishability_variants_roundtrip() {
    for fin in [
        Finishability::Proven,
        Finishability::NotProven,
        Finishability::Failed,
    ] {
        let event = make_event(EventType::PreflightComplete { finishability: fin }, "all");
        let json = serde_json::to_string(&event).expect("serialize");
        let _: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    }
}

#[test]
fn index_readiness_events_roundtrip() {
    let events = vec![
        make_event(
            EventType::IndexReadinessStarted {
                crate_name: "foo".to_string(),
                version: "1.0.0".to_string(),
            },
            "foo@1.0.0",
        ),
        make_event(
            EventType::IndexReadinessCheck {
                crate_name: "foo".to_string(),
                version: "1.0.0".to_string(),
                found: false,
            },
            "foo@1.0.0",
        ),
        make_event(
            EventType::IndexReadinessComplete {
                crate_name: "foo".to_string(),
                version: "1.0.0".to_string(),
                visible: true,
            },
            "foo@1.0.0",
        ),
    ];

    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    for e in &events {
        log.record(e.clone());
    }
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 3);
    for (orig, read) in events.iter().zip(loaded.all_events().iter()) {
        assert_eq!(orig.package, read.package);
    }
}

#[test]
fn preflight_ownership_check_roundtrip() {
    let event = make_event(
        EventType::PreflightOwnershipCheck {
            crate_name: "my-crate".to_string(),
            verified: true,
        },
        "all",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.package, "all");
}

// -- Path helper --

#[test]
fn path_helper_returns_expected_path() {
    let base = PathBuf::from("x");
    assert_eq!(events_path(&base), PathBuf::from("x").join(EVENTS_FILE));
}

#[test]
fn events_file_constant_is_events_jsonl() {
    assert_eq!(EVENTS_FILE, "events.jsonl");
}

// -- Edge cases --

#[test]
fn events_with_empty_package_string() {
    let event = make_event(EventType::ExecutionStarted, "");
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.package, "");
}

#[test]
fn events_with_unicode_in_fields() {
    let event = make_event(
        EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "échec: 失敗 🚫".to_string(),
        },
        "crâte@1.0.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.package, "crâte@1.0.0");
}

#[test]
fn large_number_of_events_roundtrip() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    for i in 0..200 {
        log.record(sample_event(&format!("pkg-{i}@0.{i}.0")));
    }
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 200);
}

#[test]
fn multiple_appends_then_single_read() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    for i in 0..5 {
        let mut log = EventLog::new();
        log.record(sample_event(&format!("pkg{i}@1.0.0")));
        log.write_to_file(&path).expect("write");
    }

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 5);
    for i in 0..5 {
        assert_eq!(loaded.all_events()[i].package, format!("pkg{i}@1.0.0"));
    }
}

#[test]
fn events_for_package_after_roundtrip() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(sample_event("a@1.0.0"));
    log.record(sample_event("b@1.0.0"));
    log.record(sample_event("a@1.0.0"));
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.events_for_package("a@1.0.0").len(), 2);
    assert_eq!(loaded.events_for_package("b@1.0.0").len(), 1);
}

#[test]
fn debug_impl_does_not_panic() {
    let log = EventLog::new();
    let debug_str = format!("{:?}", log);
    assert!(debug_str.contains("EventLog"));
}

// -- Insta snapshot tests --

#[test]
fn snapshot_package_started_event_json() {
    let event = fixed_event(
        EventType::PackageStarted {
            name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
        },
        "my-crate@1.0.0",
    );
    let json = serde_json::to_string_pretty(&event).unwrap();
    insta::assert_snapshot!("package_started_json", json);
}

#[test]
fn snapshot_package_started_event_yaml() {
    let event = fixed_event(
        EventType::PackageStarted {
            name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
        },
        "my-crate@1.0.0",
    );
    insta::assert_yaml_snapshot!("package_started_yaml", event);
}

#[test]
fn snapshot_plan_created_event_yaml() {
    let event = fixed_event(
        EventType::PlanCreated {
            plan_id: "plan-abc123".to_string(),
            package_count: 3,
        },
        "workspace",
    );
    insta::assert_yaml_snapshot!("plan_created_yaml", event);
}

#[test]
fn snapshot_execution_finished_event_yaml() {
    let event = fixed_event(
        EventType::ExecutionFinished {
            result: ExecutionResult::Success,
        },
        "workspace",
    );
    insta::assert_yaml_snapshot!("execution_finished_success_yaml", event);
}

#[test]
fn snapshot_package_failed_event_yaml() {
    let event = fixed_event(
        EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "registry returned 503".to_string(),
        },
        "my-crate@1.0.0",
    );
    insta::assert_yaml_snapshot!("package_failed_retryable_yaml", event);
}

#[test]
fn snapshot_package_published_event_yaml() {
    let event = fixed_event(
        EventType::PackagePublished { duration_ms: 4500 },
        "my-crate@1.0.0",
    );
    insta::assert_yaml_snapshot!("package_published_yaml", event);
}

#[test]
fn snapshot_readiness_complete_event_yaml() {
    let event = fixed_event(
        EventType::ReadinessComplete {
            duration_ms: 12000,
            attempts: 4,
        },
        "my-crate@1.0.0",
    );
    insta::assert_yaml_snapshot!("readiness_complete_yaml", event);
}

#[test]
fn snapshot_preflight_complete_event_yaml() {
    let event = fixed_event(
        EventType::PreflightComplete {
            finishability: Finishability::Proven,
        },
        "workspace",
    );
    insta::assert_yaml_snapshot!("preflight_complete_yaml", event);
}

#[test]
fn snapshot_multiple_events_jsonl_format() {
    let events = vec![
        fixed_event(
            EventType::PlanCreated {
                plan_id: "plan-42".to_string(),
                package_count: 2,
            },
            "workspace",
        ),
        fixed_event(EventType::ExecutionStarted, "workspace"),
        fixed_event(
            EventType::PackageStarted {
                name: "core-lib".to_string(),
                version: "0.1.0".to_string(),
            },
            "core-lib@0.1.0",
        ),
        fixed_event(
            EventType::PackagePublished { duration_ms: 3200 },
            "core-lib@0.1.0",
        ),
        fixed_event(
            EventType::ExecutionFinished {
                result: ExecutionResult::Success,
            },
            "workspace",
        ),
    ];

    let mut log = EventLog::new();
    for e in events {
        log.record(e);
    }

    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");
    log.write_to_file(&path).expect("write");

    let content = fs::read_to_string(&path).expect("read");
    insta::assert_snapshot!("multiple_events_jsonl", content);
}

#[test]
fn snapshot_event_log_roundtrip_yaml() {
    let events = vec![
        fixed_event(
            EventType::PackageStarted {
                name: "alpha".to_string(),
                version: "0.1.0".to_string(),
            },
            "alpha@0.1.0",
        ),
        fixed_event(
            EventType::PackageAttempted {
                attempt: 1,
                command: "cargo publish -p alpha".to_string(),
            },
            "alpha@0.1.0",
        ),
        fixed_event(
            EventType::PackageOutput {
                stdout_tail: "Uploading alpha v0.1.0".to_string(),
                stderr_tail: String::new(),
            },
            "alpha@0.1.0",
        ),
        fixed_event(
            EventType::PackagePublished { duration_ms: 2100 },
            "alpha@0.1.0",
        ),
    ];

    let mut log = EventLog::new();
    for e in events {
        log.record(e);
    }

    insta::assert_yaml_snapshot!("event_log_package_lifecycle", log.all_events());
}

#[test]
fn snapshot_package_skipped_event_yaml() {
    let event = fixed_event(
        EventType::PackageSkipped {
            reason: "already published".to_string(),
        },
        "old-crate@0.9.0",
    );
    insta::assert_yaml_snapshot!("package_skipped_yaml", event);
}

#[test]
fn snapshot_readiness_started_event_yaml() {
    let event = fixed_event(
        EventType::ReadinessStarted {
            method: ReadinessMethod::Api,
        },
        "my-crate@1.0.0",
    );
    insta::assert_yaml_snapshot!("readiness_started_yaml", event);
}

#[test]
fn snapshot_preflight_ownership_check_yaml() {
    let event = fixed_event(
        EventType::PreflightOwnershipCheck {
            crate_name: "my-crate".to_string(),
            verified: true,
        },
        "my-crate@1.0.0",
    );
    insta::assert_yaml_snapshot!("preflight_ownership_check_yaml", event);
}

// -- Edge-case: corrupt / truncated JSONL --

#[test]
fn read_from_file_errors_on_truncated_json() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");
    // A valid JSON object missing the closing brace
    fs::write(&path, r#"{"timestamp":"2025-01-15T12:00:00Z","event_type":{"type":"execution_started"},"package":"all"#).expect("write");
    let result = EventLog::read_from_file(&path);
    assert!(result.is_err());
}

#[test]
fn read_from_file_errors_on_binary_data() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");
    fs::write(&path, b"\x00\x01\x02\xFF\xFE\n").expect("write");
    let result = EventLog::read_from_file(&path);
    assert!(result.is_err());
}

#[test]
fn read_from_file_errors_on_empty_line_between_valid_events() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(sample_event("ok@1.0.0"));
    log.write_to_file(&path).expect("write");

    // Insert a blank line followed by another valid event
    let mut file = OpenOptions::new().append(true).open(&path).expect("open");
    writeln!(file).expect("write blank line");
    // The blank line itself should cause a parse error
    let content = fs::read_to_string(&path).expect("read");
    assert!(content.contains("\n\n"));

    let result = EventLog::read_from_file(&path);
    assert!(
        result.is_err(),
        "empty line mid-file should cause parse error"
    );
}

#[test]
fn read_from_file_errors_on_valid_json_but_wrong_schema() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");
    fs::write(&path, r#"{"name":"not-an-event","value":42}"#).expect("write");
    let result = EventLog::read_from_file(&path);
    assert!(result.is_err());
}

// -- Edge-case: very large event payloads --

#[test]
fn large_payload_over_1mb_roundtrips() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let large_string = "x".repeat(1_100_000); // >1MB
    let event = make_event(
        EventType::PackageOutput {
            stdout_tail: large_string.clone(),
            stderr_tail: "small".to_string(),
        },
        "big@1.0.0",
    );

    let mut log = EventLog::new();
    log.record(event);
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);
    match &loaded.all_events()[0].event_type {
        EventType::PackageOutput { stdout_tail, .. } => {
            assert_eq!(stdout_tail.len(), 1_100_000);
            assert_eq!(stdout_tail, &large_string);
        }
        other => panic!("unexpected event type: {other:?}"),
    }
}

#[test]
fn large_error_message_roundtrips() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let big_msg = "E".repeat(2_000_000); // 2MB
    let event = make_event(
        EventType::PackageFailed {
            class: ErrorClass::Ambiguous,
            message: big_msg.clone(),
        },
        "huge@0.1.0",
    );

    let mut log = EventLog::new();
    log.record(event);
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    match &loaded.all_events()[0].event_type {
        EventType::PackageFailed { message, .. } => assert_eq!(message, &big_msg),
        other => panic!("unexpected: {other:?}"),
    }
}

// -- Edge-case: unicode in package names and messages --

#[test]
fn unicode_cjk_package_name_roundtrips() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let event = make_event(
        EventType::PackageStarted {
            name: "日本語クレート".to_string(),
            version: "1.0.0".to_string(),
        },
        "日本語クレート@1.0.0",
    );
    let mut log = EventLog::new();
    log.record(event);
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.all_events()[0].package, "日本語クレート@1.0.0");
}

#[test]
fn unicode_emoji_in_messages_roundtrips() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let event = make_event(
        EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "🔥 error: build failed 💀 with 🚫 permissions".to_string(),
        },
        "emoji-crate@2.0.0",
    );
    let mut log = EventLog::new();
    log.record(event);
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    match &loaded.all_events()[0].event_type {
        EventType::PackageFailed { message, .. } => {
            assert!(message.contains("🔥"));
            assert!(message.contains("💀"));
            assert!(message.contains("🚫"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn unicode_combining_chars_and_rtl_roundtrip() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    // Combining characters (e + combining acute = é) and RTL Arabic
    let event = make_event(
        EventType::PackageOutput {
            stdout_tail: "cafe\u{0301} naïve résumé".to_string(),
            stderr_tail: "مرحبا بالعالم".to_string(), // Arabic "hello world"
        },
        "i18n@1.0.0",
    );
    let mut log = EventLog::new();
    log.record(event);
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    match &loaded.all_events()[0].event_type {
        EventType::PackageOutput {
            stdout_tail,
            stderr_tail,
        } => {
            assert!(stdout_tail.contains("cafe\u{0301}"));
            assert!(stderr_tail.contains("مرحبا"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

// -- Edge-case: empty events file --

#[test]
fn read_from_existing_empty_file_returns_empty_log() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");
    fs::write(&path, "").expect("create empty file");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert!(loaded.is_empty());
    assert_eq!(loaded.len(), 0);
}

#[test]
fn read_from_zero_byte_file_returns_empty_log() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");
    File::create(&path).expect("create zero-byte file");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert!(loaded.is_empty());
}

// -- Edge-case: trailing newline handling --

#[test]
fn file_with_trailing_newline_reads_correctly() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    // Write one event which produces "...\n" (trailing newline from writeln!)
    let mut log = EventLog::new();
    log.record(sample_event("a@1.0.0"));
    log.write_to_file(&path).expect("write");

    let content = fs::read_to_string(&path).expect("read");
    assert!(
        content.ends_with('\n'),
        "writeln! should produce trailing newline"
    );

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);
}

#[test]
fn file_without_trailing_newline_reads_correctly() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let event = sample_event("a@1.0.0");
    let json = serde_json::to_string(&event).expect("serialize");
    // Write without trailing newline
    fs::write(&path, &json).expect("write");

    let content = fs::read_to_string(&path).expect("read");
    assert!(!content.ends_with('\n'));

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded.all_events()[0].package, "a@1.0.0");
}

#[test]
fn file_with_multiple_trailing_newlines_errors() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(sample_event("a@1.0.0"));
    log.write_to_file(&path).expect("write");

    // Append extra blank line
    let mut file = OpenOptions::new().append(true).open(&path).expect("open");
    writeln!(file).expect("blank line");

    // The extra blank line becomes an empty string which fails JSON parse
    let result = EventLog::read_from_file(&path);
    assert!(result.is_err());
}

// -- Roundtrip serialization for every EventType variant (field-level verification) --

#[test]
fn roundtrip_plan_created_preserves_all_fields() {
    let event = fixed_event(
        EventType::PlanCreated {
            plan_id: "plan-xyz-99".to_string(),
            package_count: 42,
        },
        "workspace",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.timestamp, event.timestamp);
    assert_eq!(parsed.package, "workspace");
    match &parsed.event_type {
        EventType::PlanCreated {
            plan_id,
            package_count,
        } => {
            assert_eq!(plan_id, "plan-xyz-99");
            assert_eq!(*package_count, 42);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_execution_started_preserves_all_fields() {
    let event = fixed_event(EventType::ExecutionStarted, "ws");
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.timestamp, event.timestamp);
    assert!(matches!(parsed.event_type, EventType::ExecutionStarted));
}

#[test]
fn roundtrip_execution_finished_preserves_all_fields() {
    for result in [
        ExecutionResult::Success,
        ExecutionResult::PartialFailure,
        ExecutionResult::CompleteFailure,
    ] {
        let event = fixed_event(
            EventType::ExecutionFinished {
                result: result.clone(),
            },
            "ws",
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
        match &parsed.event_type {
            EventType::ExecutionFinished { result: r } => assert_eq!(r, &result),
            other => panic!("wrong variant: {other:?}"),
        }
    }
}

#[test]
fn roundtrip_package_started_preserves_all_fields() {
    let event = fixed_event(
        EventType::PackageStarted {
            name: "my-lib".to_string(),
            version: "3.2.1".to_string(),
        },
        "my-lib@3.2.1",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PackageStarted { name, version } => {
            assert_eq!(name, "my-lib");
            assert_eq!(version, "3.2.1");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_package_attempted_preserves_all_fields() {
    let event = fixed_event(
        EventType::PackageAttempted {
            attempt: 3,
            command: "cargo publish -p foo --no-verify".to_string(),
        },
        "foo@1.0.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PackageAttempted { attempt, command } => {
            assert_eq!(*attempt, 3);
            assert_eq!(command, "cargo publish -p foo --no-verify");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_package_output_preserves_all_fields() {
    let event = fixed_event(
        EventType::PackageOutput {
            stdout_tail: "uploading...done\n".to_string(),
            stderr_tail: "warning: unused var\n".to_string(),
        },
        "bar@0.1.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PackageOutput {
            stdout_tail,
            stderr_tail,
        } => {
            assert_eq!(stdout_tail, "uploading...done\n");
            assert_eq!(stderr_tail, "warning: unused var\n");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_package_published_preserves_all_fields() {
    let event = fixed_event(
        EventType::PackagePublished { duration_ms: 99999 },
        "z@9.0.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PackagePublished { duration_ms } => assert_eq!(*duration_ms, 99999),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_package_failed_preserves_all_fields() {
    let event = fixed_event(
        EventType::PackageFailed {
            class: ErrorClass::Ambiguous,
            message: "timeout after 30s".to_string(),
        },
        "flaky@0.1.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PackageFailed { class, message } => {
            assert_eq!(class, &ErrorClass::Ambiguous);
            assert_eq!(message, "timeout after 30s");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_package_skipped_preserves_all_fields() {
    let event = fixed_event(
        EventType::PackageSkipped {
            reason: "version already on registry".to_string(),
        },
        "old@1.0.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PackageSkipped { reason } => {
            assert_eq!(reason, "version already on registry");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_readiness_poll_preserves_all_fields() {
    let event = fixed_event(
        EventType::ReadinessPoll {
            attempt: 7,
            visible: true,
        },
        "x@1.0.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::ReadinessPoll { attempt, visible } => {
            assert_eq!(*attempt, 7);
            assert!(*visible);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_readiness_timeout_preserves_all_fields() {
    let event = fixed_event(
        EventType::ReadinessTimeout {
            max_wait_ms: 600_000,
        },
        "slow@1.0.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::ReadinessTimeout { max_wait_ms } => assert_eq!(*max_wait_ms, 600_000),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_preflight_started_preserves_all_fields() {
    let event = fixed_event(EventType::PreflightStarted, "all");
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(parsed.event_type, EventType::PreflightStarted));
}

#[test]
fn roundtrip_preflight_workspace_verify_preserves_all_fields() {
    let event = fixed_event(
        EventType::PreflightWorkspaceVerify {
            passed: false,
            output: "error[E0433]: failed to resolve".to_string(),
        },
        "all",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PreflightWorkspaceVerify { passed, output } => {
            assert!(!passed);
            assert_eq!(output, "error[E0433]: failed to resolve");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_preflight_new_crate_detected_preserves_all_fields() {
    let event = fixed_event(
        EventType::PreflightNewCrateDetected {
            crate_name: "brand-new".to_string(),
        },
        "all",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::PreflightNewCrateDetected { crate_name } => {
            assert_eq!(crate_name, "brand-new");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_index_readiness_started_preserves_all_fields() {
    let event = fixed_event(
        EventType::IndexReadinessStarted {
            crate_name: "idx".to_string(),
            version: "0.5.0".to_string(),
        },
        "idx@0.5.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::IndexReadinessStarted {
            crate_name,
            version,
        } => {
            assert_eq!(crate_name, "idx");
            assert_eq!(version, "0.5.0");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_index_readiness_check_preserves_all_fields() {
    let event = fixed_event(
        EventType::IndexReadinessCheck {
            crate_name: "idx".to_string(),
            version: "0.5.0".to_string(),
            found: false,
        },
        "idx@0.5.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::IndexReadinessCheck {
            crate_name,
            version,
            found,
        } => {
            assert_eq!(crate_name, "idx");
            assert_eq!(version, "0.5.0");
            assert!(!found);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn roundtrip_index_readiness_complete_preserves_all_fields() {
    let event = fixed_event(
        EventType::IndexReadinessComplete {
            crate_name: "idx".to_string(),
            version: "0.5.0".to_string(),
            visible: true,
        },
        "idx@0.5.0",
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let parsed: PublishEvent = serde_json::from_str(&json).expect("deserialize");
    match &parsed.event_type {
        EventType::IndexReadinessComplete {
            crate_name,
            version,
            visible,
        } => {
            assert_eq!(crate_name, "idx");
            assert_eq!(version, "0.5.0");
            assert!(visible);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

// -- Snapshot tests for missing EventType variants --

#[test]
fn snapshot_execution_started_debug() {
    let event = fixed_event(EventType::ExecutionStarted, "workspace");
    insta::assert_debug_snapshot!("execution_started_debug", event);
}

#[test]
fn snapshot_package_attempted_debug() {
    let event = fixed_event(
        EventType::PackageAttempted {
            attempt: 2,
            command: "cargo publish -p core-lib".to_string(),
        },
        "core-lib@0.1.0",
    );
    insta::assert_debug_snapshot!("package_attempted_debug", event);
}

#[test]
fn snapshot_package_output_debug() {
    let event = fixed_event(
        EventType::PackageOutput {
            stdout_tail: "Uploading core-lib v0.1.0\nFinished".to_string(),
            stderr_tail: "warning: unused import".to_string(),
        },
        "core-lib@0.1.0",
    );
    insta::assert_debug_snapshot!("package_output_debug", event);
}

#[test]
fn snapshot_readiness_poll_debug() {
    let event = fixed_event(
        EventType::ReadinessPoll {
            attempt: 3,
            visible: false,
        },
        "my-crate@1.0.0",
    );
    insta::assert_debug_snapshot!("readiness_poll_debug", event);
}

#[test]
fn snapshot_readiness_timeout_debug() {
    let event = fixed_event(
        EventType::ReadinessTimeout {
            max_wait_ms: 300000,
        },
        "my-crate@1.0.0",
    );
    insta::assert_debug_snapshot!("readiness_timeout_debug", event);
}

#[test]
fn snapshot_preflight_started_debug() {
    let event = fixed_event(EventType::PreflightStarted, "workspace");
    insta::assert_debug_snapshot!("preflight_started_debug", event);
}

#[test]
fn snapshot_preflight_workspace_verify_debug() {
    let event = fixed_event(
        EventType::PreflightWorkspaceVerify {
            passed: true,
            output: "dry-run successful".to_string(),
        },
        "workspace",
    );
    insta::assert_debug_snapshot!("preflight_workspace_verify_debug", event);
}

#[test]
fn snapshot_preflight_new_crate_detected_debug() {
    let event = fixed_event(
        EventType::PreflightNewCrateDetected {
            crate_name: "brand-new-crate".to_string(),
        },
        "workspace",
    );
    insta::assert_debug_snapshot!("preflight_new_crate_detected_debug", event);
}

#[test]
fn snapshot_index_readiness_started_debug() {
    let event = fixed_event(
        EventType::IndexReadinessStarted {
            crate_name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
        },
        "my-crate@1.0.0",
    );
    insta::assert_debug_snapshot!("index_readiness_started_debug", event);
}

#[test]
fn snapshot_index_readiness_check_debug() {
    let event = fixed_event(
        EventType::IndexReadinessCheck {
            crate_name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
            found: true,
        },
        "my-crate@1.0.0",
    );
    insta::assert_debug_snapshot!("index_readiness_check_debug", event);
}

#[test]
fn snapshot_index_readiness_complete_debug() {
    let event = fixed_event(
        EventType::IndexReadinessComplete {
            crate_name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
            visible: true,
        },
        "my-crate@1.0.0",
    );
    insta::assert_debug_snapshot!("index_readiness_complete_debug", event);
}

#[test]
fn snapshot_execution_finished_partial_failure_debug() {
    let event = fixed_event(
        EventType::ExecutionFinished {
            result: ExecutionResult::PartialFailure,
        },
        "workspace",
    );
    insta::assert_debug_snapshot!("execution_finished_partial_failure_debug", event);
}

#[test]
fn snapshot_execution_finished_complete_failure_debug() {
    let event = fixed_event(
        EventType::ExecutionFinished {
            result: ExecutionResult::CompleteFailure,
        },
        "workspace",
    );
    insta::assert_debug_snapshot!("execution_finished_complete_failure_debug", event);
}

#[test]
fn snapshot_package_failed_permanent_debug() {
    let event = fixed_event(
        EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: "crate name is reserved".to_string(),
        },
        "reserved@1.0.0",
    );
    insta::assert_debug_snapshot!("package_failed_permanent_debug", event);
}

#[test]
fn snapshot_package_failed_ambiguous_debug() {
    let event = fixed_event(
        EventType::PackageFailed {
            class: ErrorClass::Ambiguous,
            message: "connection reset during upload".to_string(),
        },
        "flaky@1.0.0",
    );
    insta::assert_debug_snapshot!("package_failed_ambiguous_debug", event);
}

// -- Snapshot: event log with all major event types --

#[test]
fn snapshot_full_publish_lifecycle_debug() {
    let events = vec![
        fixed_event(
            EventType::PlanCreated {
                plan_id: "plan-full".to_string(),
                package_count: 2,
            },
            "workspace",
        ),
        fixed_event(EventType::PreflightStarted, "workspace"),
        fixed_event(
            EventType::PreflightWorkspaceVerify {
                passed: true,
                output: "ok".to_string(),
            },
            "workspace",
        ),
        fixed_event(
            EventType::PreflightComplete {
                finishability: Finishability::Proven,
            },
            "workspace",
        ),
        fixed_event(EventType::ExecutionStarted, "workspace"),
        fixed_event(
            EventType::PackageStarted {
                name: "core".to_string(),
                version: "0.1.0".to_string(),
            },
            "core@0.1.0",
        ),
        fixed_event(
            EventType::PackageAttempted {
                attempt: 1,
                command: "cargo publish -p core".to_string(),
            },
            "core@0.1.0",
        ),
        fixed_event(
            EventType::PackagePublished { duration_ms: 1500 },
            "core@0.1.0",
        ),
        fixed_event(
            EventType::ReadinessStarted {
                method: ReadinessMethod::Api,
            },
            "core@0.1.0",
        ),
        fixed_event(
            EventType::ReadinessComplete {
                duration_ms: 3000,
                attempts: 2,
            },
            "core@0.1.0",
        ),
        fixed_event(
            EventType::PackageStarted {
                name: "cli".to_string(),
                version: "0.1.0".to_string(),
            },
            "cli@0.1.0",
        ),
        fixed_event(
            EventType::PackagePublished { duration_ms: 2000 },
            "cli@0.1.0",
        ),
        fixed_event(
            EventType::ExecutionFinished {
                result: ExecutionResult::Success,
            },
            "workspace",
        ),
    ];

    let mut log = EventLog::new();
    for e in events {
        log.record(e);
    }
    insta::assert_debug_snapshot!("full_publish_lifecycle_debug", log.all_events());
}

// -- Concurrent append from multiple threads --

#[test]
fn concurrent_appends_from_multiple_threads() {
    use std::sync::Arc;
    use std::thread;

    let td = tempdir().expect("tempdir");
    let path = Arc::new(td.path().join("events.jsonl"));
    let num_threads = 8;
    let events_per_thread = 10;

    let handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let path = Arc::clone(&path);
            thread::spawn(move || {
                for i in 0..events_per_thread {
                    let mut log = EventLog::new();
                    log.record(make_event(
                        EventType::PackagePublished {
                            duration_ms: (t * 100 + i) as u64,
                        },
                        &format!("thread{t}-pkg{i}@1.0.0"),
                    ));
                    log.write_to_file(&path).expect("write");
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread join");
    }

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), num_threads * events_per_thread);
}

// -- Additional hardening tests --

#[test]
fn single_event_roundtrip_preserves_all_data() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let event = fixed_event(
        EventType::PackagePublished { duration_ms: 42 },
        "solo@1.0.0",
    );
    let mut log = EventLog::new();
    log.record(event.clone());
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);
    let loaded_event = &loaded.all_events()[0];
    assert_eq!(loaded_event.package, "solo@1.0.0");
    assert_eq!(loaded_event.timestamp, event.timestamp);
    let json_orig = serde_json::to_string(&event).expect("ser");
    let json_loaded = serde_json::to_string(loaded_event).expect("ser");
    assert_eq!(json_orig, json_loaded);
}

#[test]
fn special_json_characters_in_payload_roundtrip() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let tricky = "quote: \" backslash: \\ tab: \t angle: <>";
    let event = make_event(
        EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: tricky.to_string(),
        },
        "tricky@1.0.0",
    );
    let mut log = EventLog::new();
    log.record(event);
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);
    match &loaded.all_events()[0].event_type {
        EventType::PackageFailed { message, .. } => assert_eq!(message, tricky),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn events_for_package_with_mixed_event_types() {
    let mut log = EventLog::new();
    let pkg = "multi@1.0.0";
    log.record(make_event(
        EventType::PackageStarted {
            name: "multi".to_string(),
            version: "1.0.0".to_string(),
        },
        pkg,
    ));
    log.record(make_event(
        EventType::PackageAttempted {
            attempt: 1,
            command: "cargo publish -p multi".to_string(),
        },
        pkg,
    ));
    log.record(make_event(
        EventType::PackagePublished { duration_ms: 500 },
        pkg,
    ));
    log.record(make_event(
        EventType::ReadinessStarted {
            method: ReadinessMethod::Api,
        },
        pkg,
    ));
    log.record(make_event(
        EventType::ReadinessComplete {
            duration_ms: 2000,
            attempts: 2,
        },
        pkg,
    ));
    log.record(make_event(
        EventType::PackageStarted {
            name: "other".to_string(),
            version: "0.1.0".to_string(),
        },
        "other@0.1.0",
    ));

    let filtered = log.events_for_package(pkg);
    assert_eq!(filtered.len(), 5);
    for e in &filtered {
        assert_eq!(e.package, pkg);
    }
}

#[test]
fn events_path_with_various_inputs() {
    assert_eq!(
        events_path(Path::new(".")),
        PathBuf::from(".").join("events.jsonl")
    );
    assert_eq!(
        events_path(Path::new("a/b/c")),
        PathBuf::from("a/b/c").join("events.jsonl")
    );
    assert_eq!(events_path(Path::new("")), PathBuf::from("events.jsonl"));
}

#[test]
fn clear_memory_does_not_affect_file() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(sample_event("first@1.0.0"));
    log.write_to_file(&path).expect("write first");

    log.clear();
    assert!(log.is_empty());

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);

    log.write_to_file(&path).expect("write empty");
    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 1);
}

#[test]
fn zero_and_max_u64_duration_roundtrip() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(make_event(
        EventType::PackagePublished { duration_ms: 0 },
        "zero@1.0.0",
    ));
    log.record(make_event(
        EventType::PackagePublished {
            duration_ms: u64::MAX,
        },
        "max@1.0.0",
    ));
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 2);
    match &loaded.all_events()[0].event_type {
        EventType::PackagePublished { duration_ms } => assert_eq!(*duration_ms, 0),
        other => panic!("unexpected: {other:?}"),
    }
    match &loaded.all_events()[1].event_type {
        EventType::PackagePublished { duration_ms } => assert_eq!(*duration_ms, u64::MAX),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn large_event_log_1000_events_filter_correctness() {
    let mut log = EventLog::new();
    for i in 0..1000 {
        let pkg = format!("pkg-{}@1.0.0", i % 10);
        log.record(sample_event(&pkg));
    }
    assert_eq!(log.len(), 1000);

    for i in 0..10 {
        let filtered = log.events_for_package(&format!("pkg-{i}@1.0.0"));
        assert_eq!(filtered.len(), 100, "filter for pkg-{i} should return 100");
    }
    assert_eq!(log.events_for_package("nonexistent").len(), 0);
}

#[test]
fn empty_strings_in_all_string_fields_roundtrip() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(make_event(
        EventType::PlanCreated {
            plan_id: String::new(),
            package_count: 0,
        },
        "",
    ));
    log.record(make_event(
        EventType::PackageStarted {
            name: String::new(),
            version: String::new(),
        },
        "",
    ));
    log.record(make_event(
        EventType::PackageOutput {
            stdout_tail: String::new(),
            stderr_tail: String::new(),
        },
        "",
    ));
    log.record(make_event(
        EventType::PackageFailed {
            class: ErrorClass::Permanent,
            message: String::new(),
        },
        "",
    ));
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 4);
    for e in loaded.all_events() {
        assert_eq!(e.package, "");
    }
}

#[test]
fn timestamp_ordering_preserved_across_append_batches() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    for batch in 0..5u32 {
        let mut log = EventLog::new();
        for i in 0..3u32 {
            log.record(make_event(
                EventType::PackagePublished {
                    duration_ms: u64::from(batch * 10 + i),
                },
                &format!("b{batch}-p{i}@1.0.0"),
            ));
        }
        log.write_to_file(&path).expect("write");
    }

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.len(), 15);
    let events = loaded.all_events();
    for i in 1..events.len() {
        assert!(
            events[i].timestamp >= events[i - 1].timestamp,
            "event {i} timestamp should be >= event {} timestamp",
            i - 1
        );
    }
}

#[test]
fn events_for_package_distinguishes_similar_names() {
    let mut log = EventLog::new();
    log.record(sample_event("foo@1.0.0"));
    log.record(sample_event("foo-bar@1.0.0"));
    log.record(sample_event("foobar@1.0.0"));
    log.record(sample_event("foo@1.0.0-rc.1"));
    log.record(sample_event("foo@1.0.0"));

    assert_eq!(log.events_for_package("foo@1.0.0").len(), 2);
    assert_eq!(log.events_for_package("foo-bar@1.0.0").len(), 1);
    assert_eq!(log.events_for_package("foobar@1.0.0").len(), 1);
    assert_eq!(log.events_for_package("foo@1.0.0-rc.1").len(), 1);
    assert_eq!(log.events_for_package("foo").len(), 0);
    assert_eq!(log.events_for_package("bar").len(), 0);
}

#[test]
fn record_after_clear_appends_only_new_events() {
    let mut log = EventLog::new();
    log.record(sample_event("old@1.0.0"));
    log.record(sample_event("old@2.0.0"));
    assert_eq!(log.len(), 2);

    log.clear();
    log.record(sample_event("new@1.0.0"));
    assert_eq!(log.len(), 1);
    assert_eq!(log.all_events()[0].package, "new@1.0.0");
}

#[test]
fn unicode_package_filter_after_file_roundtrip() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join("events.jsonl");

    let mut log = EventLog::new();
    log.record(make_event(
        EventType::PackageStarted {
            name: "日本語".to_string(),
            version: "1.0.0".to_string(),
        },
        "日本語@1.0.0",
    ));
    log.record(make_event(
        EventType::PackageStarted {
            name: "中文".to_string(),
            version: "2.0.0".to_string(),
        },
        "中文@2.0.0",
    ));
    log.record(make_event(
        EventType::PackagePublished { duration_ms: 100 },
        "日本語@1.0.0",
    ));
    log.write_to_file(&path).expect("write");

    let loaded = EventLog::read_from_file(&path).expect("read");
    assert_eq!(loaded.events_for_package("日本語@1.0.0").len(), 2);
    assert_eq!(loaded.events_for_package("中文@2.0.0").len(), 1);
}

// -- Additional snapshot tests --

#[test]
fn snapshot_package_failed_multiline_message_yaml() {
    let event = fixed_event(
        EventType::PackageFailed {
            class: ErrorClass::Retryable,
            message: "error[E0433]: failed to resolve\n  --> src/main.rs:1:5\n   |\n1  | use foo::bar;\n   |     ^^^ not found"
                .to_string(),
        },
        "broken@0.1.0",
    );
    insta::assert_yaml_snapshot!("package_failed_multiline_message_yaml", event);
}

#[test]
fn snapshot_readiness_lifecycle_debug() {
    let events = vec![
        fixed_event(
            EventType::ReadinessStarted {
                method: ReadinessMethod::Both,
            },
            "my-lib@2.0.0",
        ),
        fixed_event(
            EventType::ReadinessPoll {
                attempt: 1,
                visible: false,
            },
            "my-lib@2.0.0",
        ),
        fixed_event(
            EventType::ReadinessPoll {
                attempt: 2,
                visible: false,
            },
            "my-lib@2.0.0",
        ),
        fixed_event(
            EventType::ReadinessPoll {
                attempt: 3,
                visible: true,
            },
            "my-lib@2.0.0",
        ),
        fixed_event(
            EventType::ReadinessComplete {
                duration_ms: 9500,
                attempts: 3,
            },
            "my-lib@2.0.0",
        ),
    ];
    insta::assert_debug_snapshot!("readiness_lifecycle_debug", events);
}
