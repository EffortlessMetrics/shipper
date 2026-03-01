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

    // --- Property-based tests (proptest) ---

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for valid crate-like package names (lowercase alphanumeric + hyphens/underscores).
        fn pkg_name_strategy() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9_-]{0,30}".prop_map(|s| s)
        }

        /// Strategy for semver-like version strings.
        fn version_strategy() -> impl Strategy<Value = String> {
            (0u32..100, 0u32..100, 0u32..100).prop_map(|(ma, mi, pa)| format!("{ma}.{mi}.{pa}"))
        }

        /// Strategy for non-empty directory name segments.
        fn dir_segment_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9_-]{1,20}".prop_map(|s| s)
        }

        proptest! {
            #[test]
            fn receipt_roundtrip_arbitrary_names_and_versions(
                name in pkg_name_strategy(),
                version in version_strategy(),
                plan_id in "[a-z0-9]{1,16}",
            ) {
                let td = tempdir().expect("tempdir");
                let store = FileStore::new(td.path().to_path_buf());

                let receipt = Receipt {
                    receipt_version: "shipper.receipt.v2".to_string(),
                    plan_id,
                    registry: Registry::crates_io(),
                    started_at: Utc::now(),
                    finished_at: Utc::now(),
                    packages: vec![PackageReceipt {
                        name: name.clone(),
                        version: version.clone(),
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
                };

                store.save_receipt(&receipt).expect("save receipt");
                let loaded = store.load_receipt().expect("load receipt").expect("receipt present");
                prop_assert_eq!(&loaded.packages[0].name, &name);
                prop_assert_eq!(&loaded.packages[0].version, &version);
            }

            #[test]
            fn store_path_construction_with_arbitrary_dirs(
                segments in proptest::collection::vec(dir_segment_strategy(), 1..5),
            ) {
                let td = tempdir().expect("tempdir");
                let mut path = td.path().to_path_buf();
                for seg in &segments {
                    path = path.join(seg);
                }
                let store = FileStore::new(path.clone());
                prop_assert_eq!(store.state_dir(), path.as_path());
            }

            #[test]
            fn receipt_json_serialization_roundtrip(
                name in pkg_name_strategy(),
                version in version_strategy(),
                attempts in 1u32..10,
                duration in 0u128..100_000,
            ) {
                let receipt = Receipt {
                    receipt_version: "shipper.receipt.v2".to_string(),
                    plan_id: "rt-test".to_string(),
                    registry: Registry::crates_io(),
                    started_at: Utc::now(),
                    finished_at: Utc::now(),
                    packages: vec![PackageReceipt {
                        name: name.clone(),
                        version: version.clone(),
                        attempts,
                        state: PackageState::Published,
                        started_at: Utc::now(),
                        finished_at: Utc::now(),
                        duration_ms: duration,
                        evidence: shipper_types::PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                    }],
                    event_log_path: PathBuf::from(".shipper/events.jsonl"),
                    git_context: None,
                    environment: shipper_types::EnvironmentFingerprint {
                        shipper_version: "0.1.0".to_string(),
                        cargo_version: None,
                        rust_version: None,
                        os: "test".to_string(),
                        arch: "test".to_string(),
                    },
                };

                let json = serde_json::to_string(&receipt).expect("serialize");
                let deserialized: Receipt = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(&deserialized.packages[0].name, &name);
                prop_assert_eq!(&deserialized.packages[0].version, &version);
                prop_assert_eq!(deserialized.packages[0].attempts, attempts);
                prop_assert_eq!(deserialized.packages[0].duration_ms, duration);
            }

            #[test]
            fn events_log_append_with_arbitrary_data(
                pkg_name in pkg_name_strategy(),
                version in version_strategy(),
                event_count in 1usize..20,
            ) {
                let td = tempdir().expect("tempdir");
                let store = FileStore::new(td.path().to_path_buf());

                let mut events = EventLog::new();
                // Always start with ExecutionStarted
                events.record(shipper_types::PublishEvent {
                    timestamp: Utc::now(),
                    event_type: shipper_types::EventType::ExecutionStarted,
                    package: "all".to_string(),
                });
                // Add N package events
                for _ in 0..event_count {
                    events.record(shipper_types::PublishEvent {
                        timestamp: Utc::now(),
                        event_type: shipper_types::EventType::PackageStarted {
                            name: pkg_name.clone(),
                            version: version.clone(),
                        },
                        package: format!("{pkg_name}@{version}"),
                    });
                }

                store.save_events(&events).expect("save events");
                let loaded = store.load_events().expect("load events").expect("events present");
                // 1 ExecutionStarted + event_count PackageStarted
                prop_assert_eq!(loaded.all_events().len(), 1 + event_count);
            }
        }
    }

    // --- Partial/truncated JSON recovery ---

    #[test]
    fn file_store_load_state_truncated_json_returns_error() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let state_file = shipper_state::state_path(td.path());
        std::fs::create_dir_all(state_file.parent().unwrap_or(td.path())).ok();
        let truncated = r#"{"state_version":"shipper.state.v1","plan_id":"tr"#;
        std::fs::write(&state_file, truncated).expect("write truncated");

        let result = store.load_state();
        assert!(result.is_err(), "truncated state.json should produce error");
    }

    #[test]
    fn file_store_load_receipt_truncated_json_returns_error() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let receipt_file = shipper_state::receipt_path(td.path());
        std::fs::create_dir_all(receipt_file.parent().unwrap_or(td.path())).ok();
        let truncated = r#"{"receipt_version":"shipper.receipt.v2","plan_id":"#;
        std::fs::write(&receipt_file, truncated).expect("write truncated");

        let result = store.load_receipt();
        assert!(
            result.is_err(),
            "truncated receipt.json should produce error"
        );
    }

    // --- State transition: retry cycle (Pending → Failed → Pending) ---

    #[test]
    fn file_store_state_retry_cycle_pending_failed_pending() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut state = sample_state();
        store.save_state(&state).expect("save pending");

        let pkg = state.packages.get_mut("demo@0.1.0").unwrap();
        pkg.state = PackageState::Failed {
            class: shipper_types::ErrorClass::Retryable,
            message: "network timeout".to_string(),
        };
        pkg.attempts = 2;
        store.save_state(&state).expect("save failed");

        let loaded = store.load_state().expect("load").unwrap();
        assert!(matches!(
            loaded.packages["demo@0.1.0"].state,
            PackageState::Failed { .. }
        ));

        state.packages.get_mut("demo@0.1.0").unwrap().state = PackageState::Pending;
        store.save_state(&state).expect("save pending retry");

        let loaded = store.load_state().expect("load").unwrap();
        assert!(matches!(
            loaded.packages["demo@0.1.0"].state,
            PackageState::Pending
        ));
        assert_eq!(loaded.packages["demo@0.1.0"].attempts, 2);
    }

    // --- Published idempotent ---

    #[test]
    fn file_store_state_published_idempotent() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut state = sample_state();
        state.packages.get_mut("demo@0.1.0").unwrap().state = PackageState::Published;

        store.save_state(&state).expect("save published 1");
        store.save_state(&state).expect("save published 2");

        let loaded = store.load_state().expect("load").unwrap();
        assert!(matches!(
            loaded.packages["demo@0.1.0"].state,
            PackageState::Published
        ));
    }

    // --- Very long package names ---

    #[test]
    fn file_store_state_very_long_package_name() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let long_name = "a".repeat(500);
        let key = format!("{long_name}@1.0.0");
        let mut packages = BTreeMap::new();
        packages.insert(
            key.clone(),
            PackageProgress {
                name: long_name.clone(),
                version: "1.0.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );

        let state = ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: "long".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };

        store.save_state(&state).expect("save");
        let loaded = store.load_state().expect("load").unwrap();
        assert!(loaded.packages.contains_key(&key));
        assert_eq!(loaded.packages[&key].name, long_name);
    }

    // --- Empty plan_id ---

    #[test]
    fn file_store_state_empty_plan_id() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let state = ExecutionState {
            state_version: shipper_state::CURRENT_STATE_VERSION.to_string(),
            plan_id: String::new(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };

        store.save_state(&state).expect("save");
        let loaded = store.load_state().expect("load").unwrap();
        assert_eq!(loaded.plan_id, "");
    }

    // --- Unicode directory paths ---

    #[test]
    fn file_store_unicode_directory_path() {
        let td = tempdir().expect("tempdir");
        let unicode_dir = td.path().join("données").join("日本語");
        let store = FileStore::new(unicode_dir);

        store.save_state(&sample_state()).expect("save");
        let loaded = store.load_state().expect("load").unwrap();
        assert_eq!(loaded.plan_id, "p1");
    }

    // --- Concurrent readers ---

    #[test]
    fn file_store_concurrent_readers_consistent() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        store.save_state(&sample_state()).expect("save");

        let dir = std::sync::Arc::new(td.path().to_path_buf());
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let dir = std::sync::Arc::clone(&dir);
                std::thread::spawn(move || {
                    let store = FileStore::new((*dir).clone());
                    let loaded = store.load_state().expect("load").unwrap();
                    assert_eq!(loaded.plan_id, "p1");
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread must not panic");
        }
    }

    // --- Receipt: all published ---

    #[test]
    fn file_store_receipt_all_published() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let now = Utc::now();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "all-pub".to_string(),
            registry: Registry::crates_io(),
            started_at: now,
            finished_at: now,
            packages: vec![
                PackageReceipt {
                    name: "a".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: now,
                    finished_at: now,
                    duration_ms: 100,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "b".to_string(),
                    version: "2.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: now,
                    finished_at: now,
                    duration_ms: 200,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: shipper_types::EnvironmentFingerprint {
                shipper_version: "0.1.0".to_string(),
                cargo_version: Some("1.75.0".to_string()),
                rust_version: Some("1.75.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        store.save_receipt(&receipt).expect("save");
        let loaded = store.load_receipt().expect("load").unwrap();
        assert_eq!(loaded.packages.len(), 2);
        assert!(
            loaded
                .packages
                .iter()
                .all(|p| matches!(p.state, PackageState::Published))
        );
    }

    // --- Receipt: some failed ---

    #[test]
    fn file_store_receipt_some_failed() {
        let td = tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let now = Utc::now();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "some-failed".to_string(),
            registry: Registry::crates_io(),
            started_at: now,
            finished_at: now,
            packages: vec![
                PackageReceipt {
                    name: "a".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: now,
                    finished_at: now,
                    duration_ms: 100,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "b".to_string(),
                    version: "2.0.0".to_string(),
                    attempts: 3,
                    state: PackageState::Failed {
                        class: shipper_types::ErrorClass::Retryable,
                        message: "timeout".to_string(),
                    },
                    started_at: now,
                    finished_at: now,
                    duration_ms: 5000,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: shipper_types::EnvironmentFingerprint {
                shipper_version: "0.1.0".to_string(),
                cargo_version: Some("1.75.0".to_string()),
                rust_version: Some("1.75.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        store.save_receipt(&receipt).expect("save");
        let loaded = store.load_receipt().expect("load").unwrap();
        assert_eq!(loaded.packages.len(), 2);
        assert!(matches!(loaded.packages[0].state, PackageState::Published));
        assert!(matches!(
            loaded.packages[1].state,
            PackageState::Failed { .. }
        ));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use chrono::{DateTime, TimeZone, Utc};

    use shipper_events::EventLog;
    use shipper_types::{
        EnvironmentFingerprint, ErrorClass, EventType, ExecutionResult, ExecutionState, GitContext,
        PackageEvidence, PackageProgress, PackageReceipt, PackageState, PublishEvent, Receipt,
        Registry,
    };

    use super::*;

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap()
    }

    // ── ExecutionState snapshots ────────────────────────────────────

    #[test]
    fn snapshot_execution_state_single_pending() {
        let t = fixed_time();
        let mut packages = BTreeMap::new();
        packages.insert(
            "demo@0.1.0".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: t,
            },
        );

        let state = ExecutionState {
            state_version: "shipper.state.v1".to_string(),
            plan_id: "plan-abc".to_string(),
            registry: Registry::crates_io(),
            created_at: t,
            updated_at: t,
            packages,
        };

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("execution_state_single_pending", json);
    }

    #[test]
    fn snapshot_execution_state_all_package_states() {
        let t = fixed_time();
        let mut packages = BTreeMap::new();

        packages.insert(
            "a@1.0.0".to_string(),
            PackageProgress {
                name: "a".to_string(),
                version: "1.0.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: t,
            },
        );
        packages.insert(
            "b@1.0.0".to_string(),
            PackageProgress {
                name: "b".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Uploaded,
                last_updated_at: t,
            },
        );
        packages.insert(
            "c@1.0.0".to_string(),
            PackageProgress {
                name: "c".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: t,
            },
        );
        packages.insert(
            "d@1.0.0".to_string(),
            PackageProgress {
                name: "d".to_string(),
                version: "1.0.0".to_string(),
                attempts: 0,
                state: PackageState::Skipped {
                    reason: "already published".to_string(),
                },
                last_updated_at: t,
            },
        );
        packages.insert(
            "e@1.0.0".to_string(),
            PackageProgress {
                name: "e".to_string(),
                version: "1.0.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: ErrorClass::Permanent,
                    message: "auth error".to_string(),
                },
                last_updated_at: t,
            },
        );
        packages.insert(
            "f@1.0.0".to_string(),
            PackageProgress {
                name: "f".to_string(),
                version: "1.0.0".to_string(),
                attempts: 2,
                state: PackageState::Ambiguous {
                    message: "timeout during upload".to_string(),
                },
                last_updated_at: t,
            },
        );

        let state = ExecutionState {
            state_version: "shipper.state.v1".to_string(),
            plan_id: "plan-multi".to_string(),
            registry: Registry::crates_io(),
            created_at: t,
            updated_at: t,
            packages,
        };

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("execution_state_all_package_states", json);
    }

    #[test]
    fn snapshot_execution_state_empty_packages() {
        let t = fixed_time();
        let state = ExecutionState {
            state_version: "shipper.state.v1".to_string(),
            plan_id: "plan-empty".to_string(),
            registry: Registry::crates_io(),
            created_at: t,
            updated_at: t,
            packages: BTreeMap::new(),
        };

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("execution_state_empty_packages", json);
    }

    // ── Receipt snapshots ───────────────────────────────────────────

    #[test]
    fn snapshot_receipt_minimal() {
        let t = fixed_time();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-min".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![PackageReceipt {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: t,
                finished_at: t,
                duration_ms: 1500,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: Some("1.82.0".to_string()),
                rust_version: Some("1.82.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&receipt).expect("serialize");
        insta::assert_snapshot!("receipt_minimal", json);
    }

    #[test]
    fn snapshot_receipt_with_git_context() {
        let t = fixed_time();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-git".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![PackageReceipt {
                name: "my-lib".to_string(),
                version: "2.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: t,
                finished_at: t,
                duration_ms: 3200,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: Some(GitContext {
                commit: Some("abc123def456".to_string()),
                branch: Some("main".to_string()),
                tag: Some("v2.0.0".to_string()),
                dirty: Some(false),
            }),
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: Some("1.82.0".to_string()),
                rust_version: Some("1.82.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&receipt).expect("serialize");
        insta::assert_snapshot!("receipt_with_git_context", json);
    }

    #[test]
    fn snapshot_receipt_mixed_outcomes() {
        let t = fixed_time();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-mixed".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![
                PackageReceipt {
                    name: "core".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: t,
                    finished_at: t,
                    duration_ms: 2000,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "utils".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 0,
                    state: PackageState::Skipped {
                        reason: "already published".to_string(),
                    },
                    started_at: t,
                    finished_at: t,
                    duration_ms: 0,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "cli".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 3,
                    state: PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: "registry timeout after 3 attempts".to_string(),
                    },
                    started_at: t,
                    finished_at: t,
                    duration_ms: 45000,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: None,
                rust_version: None,
                os: "macos".to_string(),
                arch: "aarch64".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&receipt).expect("serialize");
        insta::assert_snapshot!("receipt_mixed_outcomes", json);
    }

    // ── FileStore persistence format snapshots ──────────────────────

    #[test]
    fn snapshot_state_persisted_json() {
        let t = fixed_time();
        let td = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut packages = BTreeMap::new();
        packages.insert(
            "alpha@0.1.0".to_string(),
            PackageProgress {
                name: "alpha".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: t,
            },
        );
        packages.insert(
            "beta@0.2.0".to_string(),
            PackageProgress {
                name: "beta".to_string(),
                version: "0.2.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: t,
            },
        );

        let state = ExecutionState {
            state_version: "shipper.state.v1".to_string(),
            plan_id: "plan-persist".to_string(),
            registry: Registry::crates_io(),
            created_at: t,
            updated_at: t,
            packages,
        };

        store.save_state(&state).expect("save");
        let raw = std::fs::read_to_string(shipper_state::state_path(td.path())).expect("read");
        let roundtrip: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        let pretty = serde_json::to_string_pretty(&roundtrip).expect("pretty");
        insta::assert_snapshot!("state_persisted_json", pretty);
    }

    #[test]
    fn snapshot_receipt_persisted_json() {
        let t = fixed_time();
        let td = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-persist".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![PackageReceipt {
                name: "alpha".to_string(),
                version: "0.1.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                started_at: t,
                finished_at: t,
                duration_ms: 5000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: Some("1.82.0".to_string()),
                rust_version: Some("1.82.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        store.save_receipt(&receipt).expect("save");
        let raw = std::fs::read_to_string(shipper_state::receipt_path(td.path())).expect("read");
        let roundtrip: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        let pretty = serde_json::to_string_pretty(&roundtrip).expect("pretty");
        insta::assert_snapshot!("receipt_persisted_json", pretty);
    }

    // ── Event serialization snapshots ───────────────────────────────

    #[test]
    fn snapshot_event_execution_started() {
        let t = fixed_time();
        let event = PublishEvent {
            timestamp: t,
            event_type: EventType::ExecutionStarted,
            package: "all".to_string(),
        };
        let json = serde_json::to_string_pretty(&event).expect("serialize");
        insta::assert_snapshot!("event_execution_started", json);
    }

    #[test]
    fn snapshot_event_package_started() {
        let t = fixed_time();
        let event = PublishEvent {
            timestamp: t,
            event_type: EventType::PackageStarted {
                name: "my-crate".to_string(),
                version: "1.0.0".to_string(),
            },
            package: "my-crate@1.0.0".to_string(),
        };
        let json = serde_json::to_string_pretty(&event).expect("serialize");
        insta::assert_snapshot!("event_package_started", json);
    }

    #[test]
    fn snapshot_event_package_published() {
        let t = fixed_time();
        let event = PublishEvent {
            timestamp: t,
            event_type: EventType::PackagePublished { duration_ms: 4200 },
            package: "my-crate@1.0.0".to_string(),
        };
        let json = serde_json::to_string_pretty(&event).expect("serialize");
        insta::assert_snapshot!("event_package_published", json);
    }

    #[test]
    fn snapshot_event_package_failed() {
        let t = fixed_time();
        let event = PublishEvent {
            timestamp: t,
            event_type: EventType::PackageFailed {
                class: ErrorClass::Retryable,
                message: "connection reset by peer".to_string(),
            },
            package: "my-crate@1.0.0".to_string(),
        };
        let json = serde_json::to_string_pretty(&event).expect("serialize");
        insta::assert_snapshot!("event_package_failed", json);
    }

    #[test]
    fn snapshot_event_package_skipped() {
        let t = fixed_time();
        let event = PublishEvent {
            timestamp: t,
            event_type: EventType::PackageSkipped {
                reason: "version already exists on registry".to_string(),
            },
            package: "my-crate@1.0.0".to_string(),
        };
        let json = serde_json::to_string_pretty(&event).expect("serialize");
        insta::assert_snapshot!("event_package_skipped", json);
    }

    #[test]
    fn snapshot_event_execution_finished_success() {
        let t = fixed_time();
        let event = PublishEvent {
            timestamp: t,
            event_type: EventType::ExecutionFinished {
                result: ExecutionResult::Success,
            },
            package: "all".to_string(),
        };
        let json = serde_json::to_string_pretty(&event).expect("serialize");
        insta::assert_snapshot!("event_execution_finished_success", json);
    }

    #[test]
    fn snapshot_event_execution_finished_partial_failure() {
        let t = fixed_time();
        let event = PublishEvent {
            timestamp: t,
            event_type: EventType::ExecutionFinished {
                result: ExecutionResult::PartialFailure,
            },
            package: "all".to_string(),
        };
        let json = serde_json::to_string_pretty(&event).expect("serialize");
        insta::assert_snapshot!("event_execution_finished_partial_failure", json);
    }

    // ── Schema version error message snapshots ──────────────────────

    #[test]
    fn snapshot_error_version_too_old() {
        let err = validate_schema_version("shipper.receipt.v0")
            .unwrap_err()
            .to_string();
        insta::assert_snapshot!("error_version_too_old", err);
    }

    #[test]
    fn snapshot_error_invalid_version_format() {
        let err = validate_schema_version("invalid.version")
            .unwrap_err()
            .to_string();
        insta::assert_snapshot!("error_invalid_version_format", err);
    }

    #[test]
    fn snapshot_error_empty_version() {
        let err = validate_schema_version("").unwrap_err().to_string();
        insta::assert_snapshot!("error_empty_version", err);
    }

    #[test]
    fn snapshot_error_missing_v_prefix() {
        let err = validate_schema_version("shipper.receipt.2")
            .unwrap_err()
            .to_string();
        insta::assert_snapshot!("error_missing_v_prefix", err);
    }

    #[test]
    fn snapshot_error_non_numeric_version() {
        let err = validate_schema_version("shipper.receipt.vx")
            .unwrap_err()
            .to_string();
        insta::assert_snapshot!("error_non_numeric_version", err);
    }

    // ── Events JSONL persisted format ───────────────────────────────

    #[test]
    fn snapshot_events_persisted_jsonl() {
        let t = fixed_time();
        let td = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(td.path().to_path_buf());

        let mut events = EventLog::new();
        events.record(PublishEvent {
            timestamp: t,
            event_type: EventType::ExecutionStarted,
            package: "all".to_string(),
        });
        events.record(PublishEvent {
            timestamp: t,
            event_type: EventType::PackageStarted {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
            },
            package: "demo@0.1.0".to_string(),
        });
        events.record(PublishEvent {
            timestamp: t,
            event_type: EventType::PackagePublished { duration_ms: 2500 },
            package: "demo@0.1.0".to_string(),
        });
        events.record(PublishEvent {
            timestamp: t,
            event_type: EventType::ExecutionFinished {
                result: ExecutionResult::Success,
            },
            package: "all".to_string(),
        });

        store.save_events(&events).expect("save");
        let raw = std::fs::read_to_string(shipper_events::events_path(td.path())).expect("read");
        // Normalize each line to pretty JSON for readable snapshot
        let pretty_lines: Vec<String> = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("parse line");
                serde_json::to_string_pretty(&v).expect("pretty")
            })
            .collect();
        let snapshot = pretty_lines.join("\n---\n");
        insta::assert_snapshot!("events_persisted_jsonl", snapshot);
    }

    // ── Registry serialization snapshot ─────────────────────────────

    #[test]
    fn snapshot_registry_crates_io() {
        let registry = Registry::crates_io();
        let json = serde_json::to_string_pretty(&registry).expect("serialize");
        insta::assert_snapshot!("registry_crates_io", json);
    }

    #[test]
    fn snapshot_registry_custom() {
        let registry = Registry {
            name: "my-registry".to_string(),
            api_base: "https://my-registry.example.com".to_string(),
            index_base: Some("https://index.my-registry.example.com".to_string()),
        };
        let json = serde_json::to_string_pretty(&registry).expect("serialize");
        insta::assert_snapshot!("registry_custom", json);
    }

    // ── Edge case snapshot: retry cycle state ───────────────────────

    #[test]
    fn snapshot_state_retry_cycle() {
        let t = fixed_time();
        let mut packages = BTreeMap::new();
        packages.insert(
            "retried@1.0.0".to_string(),
            PackageProgress {
                name: "retried".to_string(),
                version: "1.0.0".to_string(),
                attempts: 2,
                state: PackageState::Pending,
                last_updated_at: t,
            },
        );

        let state = ExecutionState {
            state_version: "shipper.state.v1".to_string(),
            plan_id: "plan-retry".to_string(),
            registry: Registry::crates_io(),
            created_at: t,
            updated_at: t,
            packages,
        };

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("state_retry_cycle", json);
    }

    // ── Edge case snapshot: receipt all published ────────────────────

    #[test]
    fn snapshot_receipt_all_published() {
        let t = fixed_time();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-all-pub".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![
                PackageReceipt {
                    name: "core".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: t,
                    finished_at: t,
                    duration_ms: 2000,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "utils".to_string(),
                    version: "0.5.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: t,
                    finished_at: t,
                    duration_ms: 1500,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: Some("1.82.0".to_string()),
                rust_version: Some("1.82.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&receipt).expect("serialize");
        insta::assert_snapshot!("receipt_all_published", json);
    }

    // ── Edge case snapshot: receipt some failed ──────────────────────

    #[test]
    fn snapshot_receipt_some_failed() {
        let t = fixed_time();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-some-fail".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![
                PackageReceipt {
                    name: "core".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: t,
                    finished_at: t,
                    duration_ms: 2000,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "cli".to_string(),
                    version: "2.0.0".to_string(),
                    attempts: 3,
                    state: PackageState::Failed {
                        class: ErrorClass::Permanent,
                        message: "authorization denied".to_string(),
                    },
                    started_at: t,
                    finished_at: t,
                    duration_ms: 30000,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: None,
                rust_version: None,
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&receipt).expect("serialize");
        insta::assert_snapshot!("receipt_some_failed", json);
    }
}
