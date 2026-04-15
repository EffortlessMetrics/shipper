//! Filesystem-backed implementation of [`StateStore`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::state::events::EventLog;
use crate::state::execution_state as state;
use crate::types::{ExecutionState, Receipt};

use super::StateStore;

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
        let path = crate::state::events::events_path(&self.state_dir);
        events.write_to_file(&path)
    }

    fn load_events(&self) -> Result<Option<EventLog>> {
        let path = crate::state::events::events_path(&self.state_dir);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(EventLog::read_from_file(&path)?))
    }

    fn clear(&self) -> Result<()> {
        let state_path = state::state_path(&self.state_dir);
        let receipt_path = state::receipt_path(&self.state_dir);
        let events_path = crate::state::events::events_path(&self.state_dir);

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
