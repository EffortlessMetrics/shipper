//! State persistence for shipper publish operations.
//!
//! This crate provides state management for resumable publish operations,
//! including persistence to disk and recovery of in-progress publishes.
//!
//! # Example
//!
//! ```
//! use shipper_state::{PublishState, StateStore, state_path};
//! use shipper_types::{PackageState, ErrorClass};
//! use std::path::Path;
//!
//! let mut state = PublishState::new("plan-123");
//! state.set_package_state("my-crate@1.0.0", PackageState::Published);
//!
//! // Persist state to disk
//! let store = StateStore::new(Path::new(".shipper"));
//! store.save(&state).expect("save");
//!
//! // Load state back
//! let loaded = store.load().expect("load");
//! assert_eq!(loaded.plan_id(), "plan-123");
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shipper_types::{ErrorClass, PackageState};

/// Default state file name
pub const STATE_FILE: &str = "state.json";

/// Get the state file path for a state directory
pub fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join(STATE_FILE)
}

/// Overall publish state for a plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishState {
    /// Unique identifier for this publish plan
    plan_id: String,
    /// When this state was created
    created_at: DateTime<Utc>,
    /// When this state was last updated
    updated_at: DateTime<Utc>,
    /// State of each package (keyed by "name@version")
    packages: HashMap<String, PackageState>,
    /// Number of publish attempts
    attempt_count: u32,
    /// Optional error message for the last failure
    last_error: Option<String>,
}

impl PublishState {
    /// Create a new publish state
    pub fn new(plan_id: &str) -> Self {
        let now = Utc::now();
        Self {
            plan_id: plan_id.to_string(),
            created_at: now,
            updated_at: now,
            packages: HashMap::new(),
            attempt_count: 0,
            last_error: None,
        }
    }

    /// Get the plan ID
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    /// Get when this state was created
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Get when this state was last updated
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Set the state for a package
    pub fn set_package_state(&mut self, package: &str, state: PackageState) {
        self.packages.insert(package.to_string(), state);
        self.updated_at = Utc::now();
    }

    /// Get the state for a package
    pub fn get_package_state(&self, package: &str) -> Option<&PackageState> {
        self.packages.get(package)
    }

    /// Get all package states
    pub fn packages(&self) -> &HashMap<String, PackageState> {
        &self.packages
    }

    /// Mark a package as published
    pub fn mark_published(&mut self, package: &str) {
        self.set_package_state(package, PackageState::Published);
    }

    /// Mark a package as failed
    pub fn mark_failed(&mut self, package: &str, class: ErrorClass, message: &str) {
        self.set_package_state(
            package,
            PackageState::Failed {
                class,
                message: message.to_string(),
            },
        );
        self.last_error = Some(message.to_string());
    }

    /// Mark a package as skipped
    pub fn mark_skipped(&mut self, package: &str, reason: &str) {
        self.set_package_state(package, PackageState::Skipped {
            reason: reason.to_string(),
        });
    }

    /// Increment the attempt counter
    pub fn increment_attempts(&mut self) {
        self.attempt_count += 1;
        self.updated_at = Utc::now();
    }

    /// Get the attempt count
    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// Set the last error
    pub fn set_last_error(&mut self, error: &str) {
        self.last_error = Some(error.to_string());
        self.updated_at = Utc::now();
    }

    /// Get the last error
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Check if all packages are in a terminal state (published, skipped, or failed)
    pub fn is_complete(&self) -> bool {
        self.packages.values().all(|s| matches!(
            s,
            PackageState::Published | PackageState::Skipped { .. } | PackageState::Failed { .. }
        ))
    }

    /// Get packages that still need to be processed
    pub fn pending_packages(&self) -> Vec<&str> {
        self.packages
            .iter()
            .filter(|(_, s)| matches!(s, PackageState::Pending))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Get packages that failed
    pub fn failed_packages(&self) -> Vec<&str> {
        self.packages
            .iter()
            .filter(|(_, s)| matches!(s, PackageState::Failed { .. }))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Get packages that were published successfully
    pub fn published_packages(&self) -> Vec<&str> {
        self.packages
            .iter()
            .filter(|(_, s)| matches!(s, PackageState::Published))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Clear all state (for retry from scratch)
    pub fn clear(&mut self) {
        self.packages.clear();
        self.attempt_count = 0;
        self.last_error = None;
        self.updated_at = Utc::now();
    }
}

/// Persistent store for publish state
#[derive(Debug, Clone)]
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    /// Create a new state store at the given directory
    pub fn new(state_dir: &Path) -> Self {
        Self {
            path: state_path(state_dir),
        }
    }

    /// Save state to disk
    pub fn save(&self, state: &PublishState) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create state dir {}", parent.display()))?;
        }

        // Write to temp file first, then rename for atomicity
        let tmp_path = self.path.with_extension("tmp");

        let json = serde_json::to_string_pretty(state)
            .context("failed to serialize state to JSON")?;

        fs::write(&tmp_path, json)
            .with_context(|| format!("failed to write state file {}", tmp_path.display()))?;

        fs::rename(&tmp_path, &self.path)
            .with_context(|| format!("failed to rename state file to {}", self.path.display()))?;

        Ok(())
    }

    /// Load state from disk
    pub fn load(&self) -> Result<PublishState> {
        if !self.path.exists() {
            return Err(anyhow::anyhow!("state file not found: {}", self.path.display()));
        }

        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read state file {}", self.path.display()))?;

        let state: PublishState = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse state JSON from {}", self.path.display()))?;

        Ok(state)
    }

    /// Check if state file exists
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Delete state file
    pub fn delete(&self) -> Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)
                .with_context(|| format!("failed to delete state file {}", self.path.display()))?;
        }
        Ok(())
    }

    /// Get the path to the state file
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Receipt for a completed publish operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    /// Plan ID
    pub plan_id: String,
    /// When the publish started
    pub started_at: DateTime<Utc>,
    /// When the publish completed
    pub completed_at: DateTime<Utc>,
    /// Packages that were published
    pub published: Vec<String>,
    /// Packages that were skipped
    pub skipped: Vec<String>,
    /// Packages that failed
    pub failed: Vec<String>,
    /// Total number of attempts
    pub total_attempts: u32,
    /// Whether the operation was successful
    pub success: bool,
}

impl Receipt {
    /// Create a receipt from a publish state
    pub fn from_state(state: &PublishState) -> Self {
        let published = state.published_packages().into_iter().map(String::from).collect();
        let failed = state.failed_packages().into_iter().map(String::from).collect();
        let skipped = state
            .packages()
            .iter()
            .filter(|(_, s)| matches!(s, PackageState::Skipped { .. }))
            .map(|(name, _)| name.clone())
            .collect();

        Self {
            plan_id: state.plan_id().to_string(),
            started_at: state.created_at(),
            completed_at: state.updated_at(),
            published,
            skipped,
            failed,
            total_attempts: state.attempt_count(),
            success: state.failed_packages().is_empty(),
        }
    }

    /// Get the duration of the publish operation
    pub fn duration(&self) -> chrono::Duration {
        self.completed_at - self.started_at
    }
}

/// Get the receipts file path for a state directory
pub fn receipts_path(state_dir: &Path) -> PathBuf {
    state_dir.join("receipts.jsonl")
}

/// Append a receipt to the receipts log
pub fn append_receipt(state_dir: &Path, receipt: &Receipt) -> Result<()> {
    let path = receipts_path(state_dir);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create state dir {}", parent.display()))?;
    }

    let line = serde_json::to_string(receipt)
        .context("failed to serialize receipt to JSON")?;

    // Append to file
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open receipts file {}", path.display()))?;

    use std::io::Write;
    writeln!(file, "{}", line)
        .with_context(|| format!("failed to write receipt to {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn new_state_has_plan_id() {
        let state = PublishState::new("plan-123");
        assert_eq!(state.plan_id(), "plan-123");
        assert_eq!(state.attempt_count(), 0);
        assert!(state.packages().is_empty());
    }

    #[test]
    fn set_package_state() {
        let mut state = PublishState::new("plan-123");
        state.set_package_state("crate@1.0.0", PackageState::Published);

        assert!(state.get_package_state("crate@1.0.0").is_some());
        assert!(matches!(
            state.get_package_state("crate@1.0.0"),
            Some(PackageState::Published)
        ));
    }

    #[test]
    fn mark_published() {
        let mut state = PublishState::new("plan-123");
        state.mark_published("crate@1.0.0");

        let pkg_state = state.get_package_state("crate@1.0.0").unwrap();
        assert!(matches!(pkg_state, PackageState::Published));
    }

    #[test]
    fn mark_failed() {
        let mut state = PublishState::new("plan-123");
        state.mark_failed("crate@1.0.0", ErrorClass::Permanent, "network error");

        let pkg_state = state.get_package_state("crate@1.0.0").unwrap();
        assert!(matches!(pkg_state, PackageState::Failed { .. }));
        assert_eq!(state.last_error(), Some("network error"));
    }

    #[test]
    fn mark_skipped() {
        let mut state = PublishState::new("plan-123");
        state.mark_skipped("crate@1.0.0", "already published");

        let pkg_state = state.get_package_state("crate@1.0.0").unwrap();
        if let PackageState::Skipped { reason } = pkg_state {
            assert_eq!(reason, "already published");
        } else {
            panic!("expected Skipped state");
        }
    }

    #[test]
    fn increment_attempts() {
        let mut state = PublishState::new("plan-123");
        assert_eq!(state.attempt_count(), 0);

        state.increment_attempts();
        state.increment_attempts();
        assert_eq!(state.attempt_count(), 2);
    }

    #[test]
    fn is_complete() {
        let mut state = PublishState::new("plan-123");

        // No packages is complete (nothing to do)
        assert!(state.is_complete());

        // Pending package means not complete
        state.set_package_state("crate@1.0.0", PackageState::Pending);
        assert!(!state.is_complete());

        // Published means complete
        state.mark_published("crate@1.0.0");
        assert!(state.is_complete());
    }

    #[test]
    fn pending_packages() {
        let mut state = PublishState::new("plan-123");
        state.set_package_state("a@1.0.0", PackageState::Pending);
        state.mark_published("b@1.0.0");
        state.set_package_state("c@1.0.0", PackageState::Pending);

        let pending = state.pending_packages();
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn failed_packages() {
        let mut state = PublishState::new("plan-123");
        state.mark_published("a@1.0.0");
        state.mark_failed("b@1.0.0", ErrorClass::Permanent, "error");
        state.mark_failed("c@1.0.0", ErrorClass::Retryable, "timeout");

        let failed = state.failed_packages();
        assert_eq!(failed.len(), 2);
    }

    #[test]
    fn published_packages() {
        let mut state = PublishState::new("plan-123");
        state.mark_published("a@1.0.0");
        state.mark_published("b@1.0.0");
        state.mark_failed("c@1.0.0", ErrorClass::Permanent, "error");

        let published = state.published_packages();
        assert_eq!(published.len(), 2);
    }

    #[test]
    fn state_store_save_load() {
        let td = tempdir().expect("tempdir");
        let store = StateStore::new(td.path());

        let mut state = PublishState::new("plan-123");
        state.mark_published("crate@1.0.0");
        state.increment_attempts();

        store.save(&state).expect("save");
        assert!(store.exists());

        let loaded = store.load().expect("load");
        assert_eq!(loaded.plan_id(), "plan-123");
        assert_eq!(loaded.attempt_count(), 1);
    }

    #[test]
    fn state_store_not_found() {
        let td = tempdir().expect("tempdir");
        let store = StateStore::new(td.path());

        let result = store.load();
        assert!(result.is_err());
    }

    #[test]
    fn state_store_delete() {
        let td = tempdir().expect("tempdir");
        let store = StateStore::new(td.path());

        let state = PublishState::new("plan-123");
        store.save(&state).expect("save");
        assert!(store.exists());

        store.delete().expect("delete");
        assert!(!store.exists());
    }

    #[test]
    fn receipt_from_state() {
        let mut state = PublishState::new("plan-123");
        state.mark_published("a@1.0.0");
        state.mark_skipped("b@1.0.0", "test");
        state.mark_failed("c@1.0.0", ErrorClass::Permanent, "error");
        state.increment_attempts();

        let receipt = Receipt::from_state(&state);

        assert_eq!(receipt.plan_id, "plan-123");
        assert_eq!(receipt.published.len(), 1);
        assert_eq!(receipt.skipped.len(), 1);
        assert_eq!(receipt.failed.len(), 1);
        assert!(!receipt.success);
    }

    #[test]
    fn receipt_duration() {
        let mut state = PublishState::new("plan-123");
        state.mark_published("a@1.0.0");

        let receipt = Receipt::from_state(&state);
        let duration = receipt.duration();

        // Duration should be very small for instant operation
        assert!(duration.num_milliseconds() >= 0);
    }

    #[test]
    fn append_receipt_creates_file() {
        let td = tempdir().expect("tempdir");
        let mut state = PublishState::new("plan-123");
        state.mark_published("crate@1.0.0");

        let receipt = Receipt::from_state(&state);
        append_receipt(td.path(), &receipt).expect("append");

        let path = receipts_path(td.path());
        assert!(path.exists());

        let content = fs::read_to_string(path).expect("read");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn path_helpers() {
        let base = PathBuf::from(".shipper");
        assert_eq!(state_path(&base), PathBuf::from(".shipper/state.json"));
        assert_eq!(receipts_path(&base), PathBuf::from(".shipper/receipts.jsonl"));
    }
}