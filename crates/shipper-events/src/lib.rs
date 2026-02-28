//! Event logging for shipper publish operations.
//!
//! This crate provides an append-only JSONL event log for tracking
//! publish operations, with support for package-level filtering.
//!
//! # Example
//!
//! ```
//! use shipper_events::{EventLog, events_path};
//! use shipper_types::{PublishEvent, EventType};
//! use chrono::Utc;
//! use std::path::Path;
//!
//! let mut log = EventLog::new();
//!
//! let event = PublishEvent {
//!     timestamp: Utc::now(),
//!     event_type: EventType::PackageStarted {
//!         name: "my-crate".to_string(),
//!         version: "1.0.0".to_string(),
//!     },
//!     package: "my-crate@1.0.0".to_string(),
//! };
//!
//! log.record(event);
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use shipper_types::PublishEvent;

/// Default events file name
pub const EVENTS_FILE: &str = "events.jsonl";

/// Get the events file path for a state directory
pub fn events_path(state_dir: &Path) -> PathBuf {
    state_dir.join(EVENTS_FILE)
}

/// Append-only event log for publish operations.
#[derive(Debug, Default)]
pub struct EventLog {
    events: Vec<PublishEvent>,
}

impl EventLog {
    /// Create a new empty event log.
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Record a new event.
    pub fn record(&mut self, event: PublishEvent) {
        self.events.push(event);
    }

    /// Write all recorded events to a file in JSONL format.
    ///
    /// Events are appended to the file if it already exists.
    pub fn write_to_file(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create events dir {}", parent.display()))?;
        }

        // Append mode: open file, write new events
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("failed to open events file {}", path.display()))?;

        let mut writer = std::io::BufWriter::new(file);

        for event in &self.events {
            let line = serde_json::to_string(event).context("failed to serialize event to JSON")?;
            writeln!(writer, "{}", line).context("failed to write event line")?;
        }

        writer.flush().context("failed to flush events file")?;

        Ok(())
    }

    /// Read all events from a JSONL file.
    pub fn read_from_file(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let file = File::open(path)
            .with_context(|| format!("failed to open events file {}", path.display()))?;

        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for line in reader.lines() {
            let line = line.with_context(|| {
                format!("failed to read line from events file {}", path.display())
            })?;
            let event: PublishEvent = serde_json::from_str(&line)
                .with_context(|| format!("failed to parse event JSON from line: {}", line))?;
            events.push(event);
        }

        Ok(Self { events })
    }

    /// Get all events for a specific package.
    pub fn events_for_package(&self, package: &str) -> Vec<&PublishEvent> {
        self.events
            .iter()
            .filter(|e| e.package == package)
            .collect()
    }

    /// Get all recorded events.
    pub fn all_events(&self) -> &[PublishEvent] {
        &self.events
    }

    /// Clear all recorded events from memory.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Get the number of recorded events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Check if the log is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use shipper_types::{ErrorClass, EventType, ExecutionResult, Finishability, ReadinessMethod};
    use tempfile::tempdir;

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
}

#[cfg(test)]
mod proptests {
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
    }
}
