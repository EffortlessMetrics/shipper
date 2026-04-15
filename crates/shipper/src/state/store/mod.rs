//! State store abstraction for persistence.
//!
//! Absorbed from the former `shipper-store` microcrate (Phase 2 decrating).
//! This module provides a trait-based abstraction for state storage,
//! allowing for future implementations like S3, GCS, or Azure Blob Storage.
//!
//! # Layer
//!
//! Layer 3 (`state`). Depends on `ops` (filesystem via `crate::state` helpers),
//! `events`, and `types`. Must not depend on `engine` or `plan`.

use anyhow::Result;

use crate::state::events::EventLog;
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
    shipper_schema::validate_schema_version(
        version,
        crate::state::execution_state::MINIMUM_SUPPORTED_VERSION,
        "schema",
    )
}

mod fs;
pub use fs::FileStore;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod path_edge_case_tests;
