//! State store abstraction for persistence.
//!
//! This module provides a trait-based abstraction for state storage,
//! allowing for future implementations like S3, GCS, or Azure Blob Storage.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::events::EventLog;
use crate::state;
use crate::types::{ExecutionState, Receipt};

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
    // Parse version string (e.g., "shipper.receipt.v2" -> 2)
    let version_num = parse_schema_version(version)
        .with_context(|| format!("invalid schema version format: {}", version))?;

    let minimum_num =
        parse_schema_version(crate::state::MINIMUM_SUPPORTED_VERSION).with_context(|| {
            format!(
                "invalid minimum version format: {}",
                crate::state::MINIMUM_SUPPORTED_VERSION
            )
        })?;

    if version_num < minimum_num {
        anyhow::bail!(
            "schema version {} is too old. Minimum supported version is {}",
            version,
            crate::state::MINIMUM_SUPPORTED_VERSION
        );
    }

    Ok(())
}

/// Parse schema version number from version string (e.g., "shipper.receipt.v2" -> 2)
fn parse_schema_version(version: &str) -> Result<u32> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 || !parts[0].starts_with("shipper") || !parts[2].starts_with('v') {
        anyhow::bail!("invalid schema version format: {}", version);
    }

    // Extract the version number from the last part (e.g., "v2" -> 2)
    let version_part = &parts[2][1..]; // Skip 'v'
    version_part
        .parse::<u32>()
        .with_context(|| format!("invalid version number in schema version: {}", version))
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
        state::save_state(&self.state_dir, state)
    }

    fn load_state(&self) -> Result<Option<ExecutionState>> {
        state::load_state(&self.state_dir)
    }

    fn save_receipt(&self, receipt: &Receipt) -> Result<()> {
        state::write_receipt(&self.state_dir, receipt)
    }

    fn load_receipt(&self) -> Result<Option<Receipt>> {
        state::load_receipt(&self.state_dir)
    }

    fn save_events(&self, events: &EventLog) -> Result<()> {
        let path = crate::events::events_path(&self.state_dir);
        events.write_to_file(&path)
    }

    fn load_events(&self) -> Result<Option<EventLog>> {
        let path = crate::events::events_path(&self.state_dir);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(EventLog::read_from_file(&path)?))
    }

    fn clear(&self) -> Result<()> {
        let state_path = state::state_path(&self.state_dir);
        let receipt_path = state::receipt_path(&self.state_dir);
        let events_path = crate::events::events_path(&self.state_dir);

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
    use crate::types::{PackageProgress, PackageReceipt, PackageState, Registry};
    use chrono::Utc;

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
            state_version: crate::state::CURRENT_STATE_VERSION.to_string(),
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
                evidence: crate::types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: crate::types::EnvironmentFingerprint {
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
        events.record(crate::types::PublishEvent {
            timestamp: Utc::now(),
            event_type: crate::types::EventType::ExecutionStarted,
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
        let result = parse_schema_version("shipper.receipt.v1").expect("should parse");
        assert_eq!(result, 1);
    }

    #[test]
    fn parse_schema_version_in_store_extracts_number_from_v2() {
        let result = parse_schema_version("shipper.receipt.v2").expect("should parse");
        assert_eq!(result, 2);
    }

    #[test]
    fn parse_schema_version_in_store_handles_large_version() {
        let result = parse_schema_version("shipper.receipt.v100").expect("should parse");
        assert_eq!(result, 100);
    }

    #[test]
    fn parse_schema_version_in_store_rejects_invalid_format_no_prefix() {
        let result = parse_schema_version("receipt.v2");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_in_store_rejects_invalid_format_no_version() {
        let result = parse_schema_version("shipper.receipt");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_in_store_rejects_invalid_format_missing_v() {
        let result = parse_schema_version("shipper.receipt.2");
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
}
