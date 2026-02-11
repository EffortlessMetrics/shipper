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
        let path = state::receipt_path(&self.state_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read receipt file {}", path.display()))?;
        let receipt: Receipt = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse receipt JSON {}", path.display()))?;
        Ok(Some(receipt))
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
            plan_id: "p1".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        }
    }

    fn sample_receipt() -> Receipt {
        Receipt {
            receipt_version: "shipper.receipt.v1".to_string(),
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
}
