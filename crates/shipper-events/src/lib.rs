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

    #[test]
    fn new_event_log_is_empty() {
        let log = EventLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn record_adds_event_to_log() {
        let mut log = EventLog::new();
        let event = sample_event("test@1.0.0");
        log.record(event);
        assert_eq!(log.len(), 1);
    }

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
    fn path_helper_returns_expected_path() {
        let base = PathBuf::from("x");
        assert_eq!(events_path(&base), PathBuf::from("x").join(EVENTS_FILE));
    }

    #[test]
    fn clear_removes_all_events() {
        let mut log = EventLog::new();
        log.record(sample_event("test@1.0.0"));
        log.record(sample_event("test@2.0.0"));
        assert_eq!(log.len(), 2);

        log.clear();
        assert!(log.is_empty());
    }
}
