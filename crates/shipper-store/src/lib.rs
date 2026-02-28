//! State store abstraction for persistence.
//!
//! This module provides a trait-based abstraction for state storage,
//! allowing for future implementations like S3, GCS, or Azure Blob Storage.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use shipper_events::EventLog;
use shipper_types::{ExecutionState, Receipt};

/// Trait for state storage backends.
///
/// This trait abstracts the storage of execution state, receipts, and event logs,
/// allowing for different storage backends (filesystem, S3, GCS, etc.).
pub trait StateStore: Send + Sync {
    /// Save execution state to storage
    fn save_state(&self, state: &ExecutionState) -> Result<()>;

    /// Load execution state from storage, returns None if not found
    fn load_state(&self) -> Result<Option<ExecutionState>>;

    /// Save receipt to storage
    fn save_receipt(&self, receipt: &Receipt) -> Result<()>;

    /// Load receipt from storage, returns None if not found
    fn load_receipt(&self) -> Result<Option<Receipt>>;

    /// Save event log to storage
    fn save_events(&self, events: &EventLog) -> Result<()>;

    /// Load event log from storage, returns None if not found
    fn load_events(&self) -> Result<Option<EventLog>>;

    /// Clear all state (state.json, receipt.json, events.jsonl)
    fn clear(&self) -> Result<()>;

    /// Validate schema version
    fn validate_version(&self, version: &str) -> Result<()> {
        validate_schema_version(version)
    }
}

/// Validate any schema version
pub fn validate_schema_version(version: &str) -> Result<()> {
    shipper_schema::validate_schema_version(
        version,
        shipper_state::MINIMUM_SUPPORTED_VERSION,
        "schema",
    )
}

/// Filesystem-based state store implementation.
///
/// This is the default implementation that stores state in a local directory.
pub struct FileStore {
    state_dir: PathBuf,
}

impl FileStore {
    /// Create a new FileStore with the specified state directory
    pub fn new(state_dir: PathBuf) -> Self {
        Self { state_dir }
    }

    /// Get the state directory path
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

impl StateStore for FileStore {
    fn save_state(&self, state: &ExecutionState) -> Result<()> {
        shipper_state::save_state(&self.state_dir, state)
    }

    fn load_state(&self) -> Result<Option<ExecutionState>> {
        shipper_state::load_state(&self.state_dir)
    }

    fn save_receipt(&self, receipt: &Receipt) -> Result<()> {
        shipper_state::write_receipt(&self.state_dir, receipt)
    }

    fn load_receipt(&self) -> Result<Option<Receipt>> {
        shipper_state::load_receipt(&self.state_dir)
    }

    fn save_events(&self, events: &EventLog) -> Result<()> {
        let path = shipper_events::events_path(&self.state_dir);
        events.write_to_file(&path)
    }

    fn load_events(&self) -> Result<Option<EventLog>> {
        let path = shipper_events::events_path(&self.state_dir);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(EventLog::read_from_file(&path)?))
    }

    fn clear(&self) -> Result<()> {
        let state_path = shipper_state::state_path(&self.state_dir);
        let receipt_path = shipper_state::receipt_path(&self.state_dir);
        let events_path = shipper_events::events_path(&self.state_dir);

        // Remove files if they exist
        if state_path.exists() {
            std::fs::remove_file(&state_path)
                .with_context(|| format!("failed to remove state file {}", state_path.display()))?;
        }
        if receipt_path.exists() {
            std::fs::remove_file(&receipt_path).with_context(|| {
                format!("failed to remove receipt file {}", receipt_path.display())
            })?;
        }
        if events_path.exists() {
            std::fs::remove_file(&events_path).with_context(|| {
                format!("failed to remove events file {}", events_path.display())
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    use super::*;
    use chrono::Utc;
    use shipper_types::{PackageProgress, PackageReceipt, PackageState, Registry};

    fn sample_state() -> ExecutionState {
        let mut packages = BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );

        ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "p1".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        }
    }

    fn sample_receipt() -> Receipt {
        Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "p1".to_string(),
            registry: Registry::crates_io(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            packages: vec![PackageReceipt {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 10,
                evidence: shipper_types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: shipper_types::EnvironmentFingerprint {
                shipper_version: "0.1.0".to_string(),
                cargo_version: Some("1.75.0".to_string()),
                rust_version: Some("1.75.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        }
    }

    #[test]
    fn file_store_saves_and_loads_state() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let state = sample_state();
        store.save_state(&state).expect("save state");

        let loaded = store.load_state().expect("load state");
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.plan_id, state.plan_id);
    }

    #[test]
    fn file_store_returns_none_for_missing_state() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let loaded = store.load_state().expect("load state");
        assert!(loaded.is_none());
    }

    #[test]
    fn file_store_saves_and_loads_receipt() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let receipt = sample_receipt();
        store.save_receipt(&receipt).expect("save receipt");

        let loaded = store.load_receipt().expect("load receipt");
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.plan_id, receipt.plan_id);
    }

    #[test]
    fn file_store_returns_none_for_missing_receipt() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let loaded = store.load_receipt().expect("load receipt");
        assert!(loaded.is_none());
    }

    #[test]
    fn file_store_saves_and_loads_events() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut events = EventLog::new();
        events.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::ExecutionStarted,
            package: "all".to_string(),
        });

        store.save_events(&events).expect("save events");

        let loaded = store.load_events().expect("load events");
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.all_events().len(), 1);
    }

    #[test]
    fn file_store_returns_none_for_missing_events() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let loaded = store.load_events().expect("load events");
        assert!(loaded.is_none());
    }

    #[test]
    fn file_store_clears_all_state() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        // Save some state
        store.save_state(&sample_state()).expect("save state");
        store.save_receipt(&sample_receipt()).expect("save receipt");

        // Verify it exists
        assert!(store.load_state().expect("load state").is_some());
        assert!(store.load_receipt().expect("load receipt").is_some());

        // Clear
        store.clear().expect("clear");

        // Verify it's gone
        assert!(store.load_state().expect("load state").is_none());
        assert!(store.load_receipt().expect("load receipt").is_none());
    }

    #[test]
    fn validate_schema_version_accepts_current_version() {
        let result = validate_schema_version("shipper.receipt.v2");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_schema_version_accepts_minimum_version() {
        let result = validate_schema_version("shipper.receipt.v1");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_schema_version_rejects_old_version() {
        let result = validate_schema_version("shipper.receipt.v0");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too old"));
    }

    #[test]
    fn validate_schema_version_rejects_invalid_format() {
        let result = validate_schema_version("invalid.version");
        assert!(result.is_err());
    }

    #[test]
    fn validate_schema_version_rejects_non_shipper_version() {
        let result = validate_schema_version("other.receipt.v2");
        assert!(result.is_err());
    }

    #[test]
    fn validate_schema_version_rejects_missing_version_number() {
        let result = validate_schema_version("shipper.receipt.v");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_in_store_extracts_number_from_v1() {
        let result =
            shipper_schema::parse_schema_version("shipper.receipt.v1").expect("should parse");
        assert_eq!(result, 1);
    }

    #[test]
    fn parse_schema_version_in_store_extracts_number_from_v2() {
        let result =
            shipper_schema::parse_schema_version("shipper.receipt.v2").expect("should parse");
        assert_eq!(result, 2);
    }

    #[test]
    fn parse_schema_version_in_store_handles_large_version() {
        let result =
            shipper_schema::parse_schema_version("shipper.receipt.v100").expect("should parse");
        assert_eq!(result, 100);
    }

    #[test]
    fn parse_schema_version_in_store_rejects_invalid_format_no_prefix() {
        let result = shipper_schema::parse_schema_version("receipt.v2");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_in_store_rejects_invalid_format_no_version() {
        let result = shipper_schema::parse_schema_version("shipper.receipt");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_in_store_rejects_invalid_format_missing_v() {
        let result = shipper_schema::parse_schema_version("shipper.receipt.2");
        assert!(result.is_err());
    }

    #[test]
    fn file_store_state_dir_returns_correct_path() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(".shipper");
        let store = FileStore::new(path.clone());

        assert_eq!(store.state_dir(), path);
    }

    #[test]
    fn file_store_validate_version_delegates_to_validate_schema_version() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        // Test valid version
        assert!(store.validate_version("shipper.receipt.v2").is_ok());

        // Test invalid version
        assert!(store.validate_version("shipper.receipt.v0").is_err());
    }

    // --- State transition tests ---

    #[test]
    fn file_store_state_overwrite_preserves_latest() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut state = sample_state();
        store.save_state(&state).expect("save state");

        state.plan_id = "p2".to_string();
        store.save_state(&state).expect("overwrite state");

        let loaded = store.load_state().expect("load").unwrap();
        assert_eq!(loaded.plan_id, "p2");
    }

    #[test]
    fn file_store_state_package_state_transition() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut state = sample_state();
        store.save_state(&state).expect("save pending");

        // Transition package to Published
        state.packages.get_mut("demo@0.1.0").unwrap().state = PackageState::Published;
        state.packages.get_mut("demo@0.1.0").unwrap().attempts = 2;
        store.save_state(&state).expect("save published");

        let loaded = store.load_state().expect("load").unwrap();
        let pkg = loaded.packages.get("demo@0.1.0").unwrap();
        assert!(matches!(pkg.state, PackageState::Published));
        assert_eq!(pkg.attempts, 2);
    }

    #[test]
    fn file_store_state_with_all_package_states() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut packages = BTreeMap::new();
        let now = Utc::now();

        packages.insert(
            "a@0.1.0".to_string(),
            PackageProgress {
                name: "a".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: now,
            },
        );
        packages.insert(
            "b@0.1.0".to_string(),
            PackageProgress {
                name: "b".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Uploaded,
                last_updated_at: now,
            },
        );
        packages.insert(
            "c@0.1.0".to_string(),
            PackageProgress {
                name: "c".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: now,
            },
        );
        packages.insert(
            "d@0.1.0".to_string(),
            PackageProgress {
                name: "d".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Skipped {
                    reason: "already published".to_string(),
                },
                last_updated_at: now,
            },
        );
        packages.insert(
            "e@0.1.0".to_string(),
            PackageProgress {
                name: "e".to_string(),
                version: "0.1.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: shipper_types::ErrorClass::Permanent,
                    message: "auth error".to_string(),
                },
                last_updated_at: now,
            },
        );
        packages.insert(
            "f@0.1.0".to_string(),
            PackageProgress {
                name: "f".to_string(),
                version: "0.1.0".to_string(),
                attempts: 2,
                state: PackageState::Ambiguous {
                    message: "timeout".to_string(),
                },
                last_updated_at: now,
            },
        );

        let state = ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "multi".to_string(),
            registry: Registry::crates_io(),
            created_at: now,
            updated_at: now,
            packages,
        };

        store.save_state(&state).expect("save");
        let loaded = store.load_state().expect("load").unwrap();

        assert_eq!(loaded.packages.len(), 6);
        assert!(matches!(
            loaded.packages["a@0.1.0"].state,
            PackageState::Pending
        ));
        assert!(matches!(
            loaded.packages["b@0.1.0"].state,
            PackageState::Uploaded
        ));
        assert!(matches!(
            loaded.packages["c@0.1.0"].state,
            PackageState::Published
        ));
        assert!(matches!(
            loaded.packages["d@0.1.0"].state,
            PackageState::Skipped { .. }
        ));
        assert!(matches!(
            loaded.packages["e@0.1.0"].state,
            PackageState::Failed { .. }
        ));
        assert!(matches!(
            loaded.packages["f@0.1.0"].state,
            PackageState::Ambiguous { .. }
        ));
    }

    // --- Receipt tests ---

    #[test]
    fn file_store_receipt_overwrite_preserves_latest() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut receipt = sample_receipt();
        store.save_receipt(&receipt).expect("save");

        receipt.plan_id = "p99".to_string();
        store.save_receipt(&receipt).expect("overwrite");

        let loaded = store.load_receipt().expect("load").unwrap();
        assert_eq!(loaded.plan_id, "p99");
    }

    #[test]
    fn file_store_receipt_with_git_context() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut receipt = sample_receipt();
        receipt.git_context = Some(shipper_types::GitContext {
            commit: Some("abc123".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v0.1.0".to_string()),
            dirty: Some(false),
        });

        store.save_receipt(&receipt).expect("save");
        let loaded = store.load_receipt().expect("load").unwrap();

        let ctx = loaded.git_context.expect("git_context should be Some");
        assert_eq!(ctx.commit.as_deref(), Some("abc123"));
        assert_eq!(ctx.branch.as_deref(), Some("main"));
        assert_eq!(ctx.tag.as_deref(), Some("v0.1.0"));
        assert_eq!(ctx.dirty, Some(false));
    }

    #[test]
    fn file_store_receipt_with_multiple_packages() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let now = Utc::now();
        let mut receipt = sample_receipt();
        receipt.packages.push(PackageReceipt {
            name: "lib-a".to_string(),
            version: "1.0.0".to_string(),
            attempts: 2,
            state: PackageState::Failed {
                class: shipper_types::ErrorClass::Retryable,
                message: "network timeout".to_string(),
            },
            started_at: now,
            finished_at: now,
            duration_ms: 5000,
            evidence: shipper_types::PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
        });

        store.save_receipt(&receipt).expect("save");
        let loaded = store.load_receipt().expect("load").unwrap();
        assert_eq!(loaded.packages.len(), 2);
        assert_eq!(loaded.packages[1].name, "lib-a");
        assert!(matches!(
            loaded.packages[1].state,
            PackageState::Failed { .. }
        ));
    }

    // --- Events tests ---

    #[test]
    fn file_store_events_multiple_entries_roundtrip() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut events = EventLog::new();
        events.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::ExecutionStarted,
            package: "all".to_string(),
        });
        events.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::PackageStarted {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
            },
            package: "demo".to_string(),
        });
        events.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::PackagePublished { duration_ms: 1500 },
            package: "demo".to_string(),
        });
        events.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::ExecutionFinished {
                result: shipper_types::ExecutionResult::Success,
            },
            package: "all".to_string(),
        });

        store.save_events(&events).expect("save events");
        let loaded = store.load_events().expect("load events").unwrap();
        assert_eq!(loaded.all_events().len(), 4);
    }

    #[test]
    fn file_store_events_overwrite() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut events = EventLog::new();
        events.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::ExecutionStarted,
            package: "all".to_string(),
        });
        store.save_events(&events).expect("first save");

        // Save different events
        let mut events2 = EventLog::new();
        events2.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::PreflightStarted,
            package: "all".to_string(),
        });
        events2.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::PreflightComplete {
                finishability: shipper_types::Finishability::Proven,
            },
            package: "all".to_string(),
        });
        store.save_events(&events2).expect("second save");

        let loaded = store.load_events().expect("load").unwrap();
        // EventLog::write_to_file appends, so we get all events
        assert!(loaded.all_events().len() >= 2);
    }

    // --- Clear / edge-case tests ---

    #[test]
    fn file_store_clear_on_empty_store_succeeds() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        // Clearing when nothing was saved should succeed
        store.clear().expect("clear on empty store");
    }

    #[test]
    fn file_store_clear_is_idempotent() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        store.save_state(&sample_state()).expect("save");
        store.clear().expect("first clear");
        store.clear().expect("second clear");

        assert!(store.load_state().expect("load").is_none());
    }

    #[test]
    fn file_store_clear_removes_events_too() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut events = EventLog::new();
        events.record(shipper_types::PublishEvent {
            timestamp: Utc::now(),
            event_type: shipper_types::EventType::ExecutionStarted,
            package: "all".to_string(),
        });
        store.save_events(&events).expect("save events");
        store.save_state(&sample_state()).expect("save state");
        store.save_receipt(&sample_receipt()).expect("save receipt");

        store.clear().expect("clear");

        assert!(store.load_state().expect("load state").is_none());
        assert!(store.load_receipt().expect("load receipt").is_none());
        assert!(store.load_events().expect("load events").is_none());
    }

    #[test]
    fn file_store_clear_does_not_remove_other_files() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        // Create an unrelated file in the state dir
        let other_file = td.path().join("other.txt");
        std::fs::write(&other_file, "keep me").expect("write other file");

        store.save_state(&sample_state()).expect("save");
        store.clear().expect("clear");

        assert!(other_file.exists(), "unrelated file should not be removed");
    }

    // --- Corrupt / invalid data tests ---

    #[test]
    fn file_store_load_state_corrupt_json_returns_error() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let state_file = shipper_state::state_path(td.path());
        std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
        std::fs::write(&state_file, "{ not valid json !!!").expect("write corrupt");

        let result = store.load_state();
        assert!(result.is_err(), "corrupt state.json should produce error");
    }

    #[test]
    fn file_store_load_receipt_corrupt_json_returns_error() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let receipt_file = shipper_state::receipt_path(td.path());
        std::fs::create_dir_all(receipt_file.parent().unwrap_or(td.path())).ok();
        std::fs::write(&receipt_file, "<<<garbage>>>").expect("write corrupt");

        let result = store.load_receipt();
        assert!(result.is_err(), "corrupt receipt.json should produce error");
    }

    #[test]
    fn file_store_load_events_corrupt_jsonl_returns_error() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let events_file = shipper_events::events_path(td.path());
        std::fs::create_dir_all(events_file.parent().unwrap_or(td.path())).ok();
        std::fs::write(&events_file, "not-json-at-all\n").expect("write corrupt");

        let result = store.load_events();
        assert!(result.is_err(), "corrupt events.jsonl should produce error");
    }

    #[test]
    fn file_store_load_state_empty_file_returns_error() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let state_file = shipper_state::state_path(td.path());
        std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
        std::fs::write(&state_file, "").expect("write empty");

        let result = store.load_state();
        assert!(result.is_err(), "empty state.json should produce error");
    }

    // --- Roundtrip fidelity tests ---

    #[test]
    fn file_store_state_roundtrip_preserves_all_fields() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let state = sample_state();
        store.save_state(&state).expect("save");
        let loaded = store.load_state().expect("load").unwrap();

        assert_eq!(loaded.state_version, state.state_version);
        assert_eq!(loaded.plan_id, state.plan_id);
        assert_eq!(loaded.registry.name, state.registry.name);
        assert_eq!(loaded.packages.len(), state.packages.len());

        let orig_pkg = state.packages.get("demo@0.1.0").unwrap();
        let load_pkg = loaded.packages.get("demo@0.1.0").unwrap();
        assert_eq!(load_pkg.name, orig_pkg.name);
        assert_eq!(load_pkg.version, orig_pkg.version);
        assert_eq!(load_pkg.attempts, orig_pkg.attempts);
    }

    #[test]
    fn file_store_receipt_roundtrip_preserves_all_fields() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let receipt = sample_receipt();
        store.save_receipt(&receipt).expect("save");
        let loaded = store.load_receipt().expect("load").unwrap();

        assert_eq!(loaded.receipt_version, receipt.receipt_version);
        assert_eq!(loaded.plan_id, receipt.plan_id);
        assert_eq!(loaded.registry.name, receipt.registry.name);
        assert_eq!(loaded.packages.len(), receipt.packages.len());
        assert_eq!(loaded.packages[0].name, receipt.packages[0].name);
        assert_eq!(loaded.packages[0].version, receipt.packages[0].version);
        assert_eq!(loaded.packages[0].attempts, receipt.packages[0].attempts);
        assert_eq!(
            loaded.packages[0].duration_ms,
            receipt.packages[0].duration_ms
        );
        assert_eq!(
            loaded.environment.shipper_version,
            receipt.environment.shipper_version
        );
        assert_eq!(loaded.environment.os, receipt.environment.os);
        assert_eq!(loaded.event_log_path, receipt.event_log_path);
    }

    // --- Empty packages edge case ---

    #[test]
    fn file_store_state_with_empty_packages() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let state = ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "empty".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };

        store.save_state(&state).expect("save empty");
        let loaded = store.load_state().expect("load").unwrap();
        assert!(loaded.packages.is_empty());
        assert_eq!(loaded.plan_id, "empty");
    }

    #[test]
    fn file_store_receipt_with_empty_packages() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut receipt = sample_receipt();
        receipt.packages.clear();

        store.save_receipt(&receipt).expect("save");
        let loaded = store.load_receipt().expect("load").unwrap();
        assert!(loaded.packages.is_empty());
    }

    #[test]
    fn file_store_empty_event_log_roundtrip() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let events = EventLog::new();
        store.save_events(&events).expect("save empty events");

        let loaded = store.load_events().expect("load");
        // Empty event log may write an empty file; implementation may return Some or None
        if let Some(loaded) = loaded {
            assert!(loaded.all_events().is_empty());
        }
    }

    // --- Schema version edge cases ---

    #[test]
    fn validate_schema_version_rejects_empty_string() {
        assert!(validate_schema_version("").is_err());
    }

    #[test]
    fn validate_schema_version_rejects_future_version_gracefully() {
        // Future versions should be accepted (forward compatible)
        let result = validate_schema_version("shipper.receipt.v999");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_schema_version_rejects_negative_looking_version() {
        let result = validate_schema_version("shipper.receipt.v-1");
        assert!(result.is_err());
    }

    // --- StateStore trait as trait object ---

    #[test]
    fn file_store_usable_as_dyn_state_store() {
        let td = tempdir().expect("tempdir");
        let store: Box<dyn StateStore> = Box::new(FileStore::new(td.path().to_path_buf()));

        store
            .save_state(&sample_state())
            .expect("save via trait object");
        let loaded = store.load_state().expect("load via trait object");
        assert!(loaded.is_some());
    }

    // --- Save to non-existent nested directory ---

    #[test]
    fn file_store_save_creates_parent_directories() {
        let td = tempdir().expect("tempdir");
        let nested = td.path().join("deep").join("nested").join(".shipper");
        let store = FileStore::new(nested);

        // save_state should create directories as needed
        let result = store.save_state(&sample_state());
        assert!(result.is_ok(), "save should create parent dirs: {result:?}");

        let loaded = store.load_state().expect("load").unwrap();
        assert_eq!(loaded.plan_id, "p1");
    }
}
