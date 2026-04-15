//! Execution state and receipt persistence (atomic write + schema-versioned migration).
//!
//! **Layer:** state (layer 3).
//!
//! Absorbed from the former `shipper-state` microcrate (Phase 2 decrating).
//! This module provides atomic persistence for [`ExecutionState`] and
//! [`Receipt`] with schema-versioned migration.
//!
//! # Invariants
//!
//! - Writes are atomic: write to `.tmp` sibling, `sync_all`, then rename.
//! - Forward-compatible schema: unknown receipt versions are best-effort
//!   deserialized.
//! - v1 → v2 receipt migration fills missing `git_context` (null) and
//!   `environment` fields and rewrites `receipt_version`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::runtime::environment::collect_environment_fingerprint;
use shipper_types::{ExecutionState, Receipt};

#[cfg(test)]
mod tests;

/// Current receipt schema version
pub const CURRENT_RECEIPT_VERSION: &str = "shipper.receipt.v2";

/// Minimum supported receipt schema version
pub const MINIMUM_SUPPORTED_VERSION: &str = "shipper.receipt.v1";

/// Current state schema version
pub const CURRENT_STATE_VERSION: &str = "shipper.state.v1";

/// Current plan schema version
pub const CURRENT_PLAN_VERSION: &str = "shipper.plan.v1";

pub const STATE_FILE: &str = "state.json";
pub const RECEIPT_FILE: &str = "receipt.json";

pub fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join(STATE_FILE)
}

pub fn receipt_path(state_dir: &Path) -> PathBuf {
    state_dir.join(RECEIPT_FILE)
}

pub fn load_state(state_dir: &Path) -> Result<Option<ExecutionState>> {
    let path = state_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read state file {}", path.display()))?;
    let st: ExecutionState = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse state JSON {}", path.display()))?;
    Ok(Some(st))
}

pub fn save_state(state_dir: &Path, state: &ExecutionState) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

    let path = state_path(state_dir);
    atomic_write_json(&path, state)
}

pub fn write_receipt(state_dir: &Path, receipt: &Receipt) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

    let path = receipt_path(state_dir);
    atomic_write_json(&path, receipt)
}

/// Clear state file (state.json) from state directory
pub fn clear_state(state_dir: &Path) -> Result<()> {
    let path = state_path(state_dir);
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove state file {}", path.display()))?;
    }
    Ok(())
}

/// Check if there's incomplete state (state.json exists but receipt.json doesn't)
pub fn has_incomplete_state(state_dir: &Path) -> bool {
    state_path(state_dir).exists() && !receipt_path(state_dir).exists()
}

/// Load state with encryption support
pub fn load_state_encrypted(
    state_dir: &Path,
    encrypt_config: &shipper_encrypt::EncryptionConfig,
) -> Result<Option<ExecutionState>> {
    let path = state_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }

    let encryption = shipper_encrypt::StateEncryption::new(encrypt_config.clone())?;
    let content = encryption.read_file(&path)?;

    let st: ExecutionState = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse state JSON {}", path.display()))?;
    Ok(Some(st))
}

/// Save state with encryption support
pub fn save_state_encrypted(
    state_dir: &Path,
    state: &ExecutionState,
    encrypt_config: &shipper_encrypt::EncryptionConfig,
) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

    let path = state_path(state_dir);

    let encryption = shipper_encrypt::StateEncryption::new(encrypt_config.clone())?;
    let data = serde_json::to_vec_pretty(state).context("failed to serialize state JSON")?;
    encryption.write_file(&path, &data)
}

/// Write receipt with encryption support
pub fn write_receipt_encrypted(
    state_dir: &Path,
    receipt: &Receipt,
    encrypt_config: &shipper_encrypt::EncryptionConfig,
) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

    let path = receipt_path(state_dir);

    let encryption = shipper_encrypt::StateEncryption::new(encrypt_config.clone())?;
    let data = serde_json::to_vec_pretty(receipt).context("failed to serialize receipt JSON")?;
    encryption.write_file(&path, &data)
}

/// Load receipt with encryption support
pub fn load_receipt_encrypted(
    state_dir: &Path,
    encrypt_config: &shipper_encrypt::EncryptionConfig,
) -> Result<Option<Receipt>> {
    let path = receipt_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }

    let encryption = shipper_encrypt::StateEncryption::new(encrypt_config.clone())?;
    let content = encryption.read_file(&path)?;

    // Try to parse as Receipt directly
    if let Ok(receipt) = serde_json::from_str::<Receipt>(&content) {
        // Validate the version
        if let Err(_e) = validate_receipt_version(&receipt.receipt_version) {
            // If version is too old, attempt migration
            // Note: migration requires raw file access, so we'll handle this case separately
            return migrate_receipt_encrypted(&path, encrypt_config).map(Some);
        }
        return Ok(Some(receipt));
    }

    // If direct parsing failed, attempt migration
    migrate_receipt_encrypted(&path, encrypt_config).map(Some)
}

/// Migrate receipt with encryption support
fn migrate_receipt_encrypted(
    path: &Path,
    encrypt_config: &shipper_encrypt::EncryptionConfig,
) -> Result<Receipt> {
    let encryption = shipper_encrypt::StateEncryption::new(encrypt_config.clone())?;
    let content = encryption.read_file(path)?;

    let value: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse receipt JSON {}", path.display()))?;

    let receipt_version = value
        .get("receipt_version")
        .and_then(|v| v.as_str())
        .unwrap_or("shipper.receipt.v1")
        .to_string();

    validate_receipt_version(&receipt_version)?;

    let receipt = match receipt_version.as_str() {
        "shipper.receipt.v1" => migrate_v1_to_v2(value)?,
        "shipper.receipt.v2" => serde_json::from_value(value)
            .with_context(|| format!("failed to deserialize receipt v2 from {}", path.display()))?,
        _ => serde_json::from_value(value).with_context(|| {
            format!(
                "failed to deserialize receipt with unknown version {} from {}",
                receipt_version,
                path.display()
            )
        })?,
    };

    Ok(receipt)
}

/// Validate receipt schema version
pub fn validate_receipt_version(version: &str) -> Result<()> {
    shipper_types::schema::validate_schema_version(version, MINIMUM_SUPPORTED_VERSION, "receipt")
}

/// Migrate a receipt from an older schema version to the current version
pub fn migrate_receipt(path: &Path) -> Result<Receipt> {
    // Load the receipt JSON
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read receipt file {}", path.display()))?;

    let value: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse receipt JSON {}", path.display()))?;

    // Check the receipt_version field
    let receipt_version = value
        .get("receipt_version")
        .and_then(|v| v.as_str())
        .unwrap_or("shipper.receipt.v1") // Default to v1 if missing
        .to_string(); // Clone to avoid borrow issues

    // Validate the version
    validate_receipt_version(&receipt_version)?;

    // Apply migrations based on version
    let receipt = match receipt_version.as_str() {
        "shipper.receipt.v1" => migrate_v1_to_v2(value)?,
        "shipper.receipt.v2" => serde_json::from_value(value)
            .with_context(|| format!("failed to deserialize receipt v2 from {}", path.display()))?,
        _ => {
            // Unknown version - try to deserialize anyway (may fail on unknown fields)
            serde_json::from_value(value).with_context(|| {
                format!(
                    "failed to deserialize receipt with unknown version {} from {}",
                    receipt_version,
                    path.display()
                )
            })?
        }
    };

    Ok(receipt)
}

/// Migrate v1 receipt to v2
fn migrate_v1_to_v2(mut receipt: serde_json::Value) -> Result<Receipt> {
    // Add git_context: None if not present
    if receipt.get("git_context").is_none() {
        receipt["git_context"] = serde_json::Value::Null;
    }

    // Add environment: default EnvironmentFingerprint if not present
    if receipt.get("environment").is_none() {
        let environment = collect_environment_fingerprint();
        receipt["environment"] = serde_json::to_value(environment)
            .context("failed to serialize environment fingerprint")?;
    }

    // Update receipt_version to v2
    receipt["receipt_version"] = serde_json::Value::String(CURRENT_RECEIPT_VERSION.to_string());

    // Deserialize as Receipt
    serde_json::from_value(receipt).context("failed to deserialize migrated receipt")
}

/// Load receipt from state directory with migration support
pub fn load_receipt(state_dir: &Path) -> Result<Option<Receipt>> {
    let path = receipt_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }

    // Try to load directly first
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read receipt file {}", path.display()))?;

    // Try to parse as Receipt directly
    if let Ok(receipt) = serde_json::from_str::<Receipt>(&content) {
        // Validate the version
        if let Err(_e) = validate_receipt_version(&receipt.receipt_version) {
            // If version is too old, attempt migration
            return migrate_receipt(&path).map(Some);
        }
        return Ok(Some(receipt));
    }

    // If direct parsing failed, attempt migration
    migrate_receipt(&path).map(Some)
}

/// Best-effort fsync of the parent directory after a rename, ensuring the
/// directory entry update is durable on crash.  Errors are silently ignored
/// because not all platforms support opening a directory for sync (e.g. Windows).
pub fn fsync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let data = serde_json::to_vec_pretty(value).context("failed to serialize JSON")?;

    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("failed to create tmp file {}", tmp.display()))?;
        f.write_all(&data)
            .with_context(|| format!("failed to write tmp file {}", tmp.display()))?;
        f.sync_all().ok();
    }

    fs::rename(&tmp, path).with_context(|| {
        format!(
            "failed to rename tmp file {} to {}",
            tmp.display(),
            path.display()
        )
    })?;

    fsync_parent_dir(path);

    Ok(())
}
