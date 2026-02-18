use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::encryption::StateEncryption;
use crate::environment::collect_environment_fingerprint;
use crate::types::{ExecutionState, Receipt};

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
pub fn load_state_encrypted(state_dir: &Path, encrypt_config: &crate::encryption::EncryptionConfig) -> Result<Option<ExecutionState>> {
    let path = state_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }
    
    let encryption = crate::encryption::StateEncryption::new(encrypt_config.clone())?;
    let content = encryption.read_file(&path)?;
    
    let st: ExecutionState = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse state JSON {}", path.display()))?;
    Ok(Some(st))
}

/// Save state with encryption support
pub fn save_state_encrypted(state_dir: &Path, state: &ExecutionState, encrypt_config: &crate::encryption::EncryptionConfig) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

    let path = state_path(state_dir);
    
    let encryption = crate::encryption::StateEncryption::new(encrypt_config.clone())?;
    let data = serde_json::to_vec_pretty(state).context("failed to serialize state JSON")?;
    encryption.write_file(&path, &data)
}

/// Write receipt with encryption support
pub fn write_receipt_encrypted(state_dir: &Path, receipt: &Receipt, encrypt_config: &crate::encryption::EncryptionConfig) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

    let path = receipt_path(state_dir);
    
    let encryption = crate::encryption::StateEncryption::new(encrypt_config.clone())?;
    let data = serde_json::to_vec_pretty(receipt).context("failed to serialize receipt JSON")?;
    encryption.write_file(&path, &data)
}

/// Load receipt with encryption support
pub fn load_receipt_encrypted(state_dir: &Path, encrypt_config: &crate::encryption::EncryptionConfig) -> Result<Option<Receipt>> {
    let path = receipt_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }

    let encryption = crate::encryption::StateEncryption::new(encrypt_config.clone())?;
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
fn migrate_receipt_encrypted(path: &Path, encrypt_config: &crate::encryption::EncryptionConfig) -> Result<Receipt> {
    let encryption = crate::encryption::StateEncryption::new(encrypt_config.clone())?;
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
        _ => {
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

/// Validate receipt schema version
pub fn validate_receipt_version(version: &str) -> Result<()> {
    // Parse version string (e.g., "shipper.receipt.v2" -> 2)
    let version_num = parse_schema_version(version)
        .with_context(|| format!("invalid receipt version format: {}", version))?;

    let minimum_num = parse_schema_version(MINIMUM_SUPPORTED_VERSION).with_context(|| {
        format!(
            "invalid minimum version format: {}",
            MINIMUM_SUPPORTED_VERSION
        )
    })?;

    if version_num < minimum_num {
        anyhow::bail!(
            "receipt version {} is too old. Minimum supported version is {}",
            version,
            MINIMUM_SUPPORTED_VERSION
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
pub(crate) fn fsync_parent_dir(path: &Path) {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::types::{
        ExecutionState, PackageProgress, PackageReceipt, PackageState, Receipt, Registry,
    };

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
            state_version: CURRENT_STATE_VERSION.to_string(),
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
    fn path_helpers_append_expected_files() {
        let base = PathBuf::from("x");
        assert_eq!(state_path(&base), PathBuf::from("x").join(STATE_FILE));
        assert_eq!(receipt_path(&base), PathBuf::from("x").join(RECEIPT_FILE));
    }

    #[test]
    fn load_state_returns_none_when_file_missing() {
        let td = tempdir().expect("tempdir");
        let loaded = load_state(td.path()).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn save_and_load_state_roundtrip() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("nested").join("state");
        let st = sample_state();

        save_state(&dir, &st).expect("save state");
        let loaded = load_state(&dir).expect("load state").expect("exists");

        assert_eq!(loaded.plan_id, st.plan_id);
        assert_eq!(loaded.registry.name, st.registry.name);
        assert_eq!(loaded.packages.len(), 1);
    }

    #[test]
    fn write_receipt_creates_file() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        let receipt = sample_receipt();

        write_receipt(&dir, &receipt).expect("write receipt");
        let path = receipt_path(&dir);
        let content = fs::read_to_string(path).expect("read receipt");
        assert!(content.contains("\"receipt_version\""));
        assert!(content.contains("\"shipper.receipt.v2\""));
    }

    #[test]
    fn validate_receipt_version_accepts_current_version() {
        validate_receipt_version(CURRENT_RECEIPT_VERSION).expect("current version should be valid");
    }

    #[test]
    fn validate_receipt_version_accepts_minimum_version() {
        validate_receipt_version(MINIMUM_SUPPORTED_VERSION)
            .expect("minimum version should be valid");
    }

    #[test]
    fn validate_receipt_version_rejects_old_version() {
        let result = validate_receipt_version("shipper.receipt.v0");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too old"));
    }

    #[test]
    fn validate_receipt_version_rejects_invalid_format() {
        let result = validate_receipt_version("invalid.version");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_extracts_number() {
        let result = parse_schema_version("shipper.receipt.v2").expect("should parse");
        assert_eq!(result, 2);
    }

    #[test]
    fn parse_schema_version_handles_single_digit() {
        let result = parse_schema_version("shipper.receipt.v1").expect("should parse");
        assert_eq!(result, 1);
    }

    #[test]
    fn parse_schema_version_handles_large_version() {
        let result = parse_schema_version("shipper.receipt.v100").expect("should parse");
        assert_eq!(result, 100);
    }

    #[test]
    fn parse_schema_version_rejects_invalid_format() {
        let result = parse_schema_version("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn migrate_v1_to_v2_adds_missing_fields() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("receipt.json");

        // Create a v1 receipt (without git_context and environment)
        let v1_json = r#"{
            "receipt_version": "shipper.receipt.v1",
            "plan_id": "test-plan",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T01:00:00Z",
            "packages": [],
            "event_log_path": ".shipper/events.jsonl"
        }"#;

        fs::write(&path, v1_json).expect("write v1 receipt");

        let receipt = migrate_receipt(&path).expect("migrate receipt");

        assert_eq!(receipt.receipt_version, CURRENT_RECEIPT_VERSION);
        assert!(receipt.git_context.is_none());
        assert!(!receipt.environment.shipper_version.is_empty());
    }

    #[test]
    fn load_receipt_migrates_v1_to_v2() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        let path = receipt_path(&dir);

        // Create a v1 receipt
        let v1_json = r#"{
            "receipt_version": "shipper.receipt.v1",
            "plan_id": "test-plan",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T01:00:00Z",
            "packages": [],
            "event_log_path": ".shipper/events.jsonl"
        }"#;

        fs::write(&path, v1_json).expect("write v1 receipt");

        let receipt = load_receipt(&dir)
            .expect("load receipt")
            .expect("receipt exists");

        assert_eq!(receipt.receipt_version, CURRENT_RECEIPT_VERSION);
        assert!(receipt.git_context.is_none());
        assert!(!receipt.environment.shipper_version.is_empty());
    }

    #[test]
    fn load_receipt_loads_v2_directly() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        let receipt = sample_receipt();

        write_receipt(&dir, &receipt).expect("write receipt");

        let loaded = load_receipt(&dir)
            .expect("load receipt")
            .expect("receipt exists");

        assert_eq!(loaded.receipt_version, receipt.receipt_version);
        assert_eq!(loaded.plan_id, receipt.plan_id);
    }

    #[test]
    fn load_state_fails_on_invalid_json() {
        let td = tempdir().expect("tempdir");
        let path = state_path(td.path());
        fs::create_dir_all(td.path()).expect("mkdir");
        fs::write(&path, "{not-json").expect("write");

        let err = load_state(td.path()).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("failed to parse state JSON"));
    }

    #[test]
    fn save_state_surfaces_rename_error() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("state-dir");
        fs::create_dir_all(&dir).expect("mkdir");

        // Force `rename(tmp, state.json)` to fail by pre-creating state.json as a directory.
        fs::create_dir_all(state_path(&dir)).expect("mkdir conflicting state path");

        let err = save_state(&dir, &sample_state()).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to rename tmp file"));
    }

    #[test]
    fn validate_receipt_version_rejects_non_shipper_version() {
        let result = validate_receipt_version("other.receipt.v2");
        assert!(result.is_err());
    }

    #[test]
    fn validate_receipt_version_rejects_missing_version_number() {
        let result = validate_receipt_version("shipper.receipt.v");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_rejects_invalid_format_no_prefix() {
        let result = parse_schema_version("receipt.v2");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_rejects_invalid_format_no_version() {
        let result = parse_schema_version("shipper.receipt");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_rejects_invalid_format_missing_v() {
        let result = parse_schema_version("shipper.receipt.2");
        assert!(result.is_err());
    }

    #[test]
    fn migrate_v1_to_v2_adds_git_context_as_none() {
        let v1_json = serde_json::json!({
            "receipt_version": "shipper.receipt.v1",
            "plan_id": "test-plan",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T01:00:00Z",
            "packages": [],
            "event_log_path": ".shipper/events.jsonl"
        });

        let receipt = migrate_v1_to_v2(v1_json).expect("migrate receipt");

        assert_eq!(receipt.receipt_version, CURRENT_RECEIPT_VERSION);
        assert!(receipt.git_context.is_none());
        assert!(!receipt.environment.shipper_version.is_empty());
    }

    #[test]
    fn migrate_v1_to_v2_preserves_existing_git_context() {
        let v1_json = serde_json::json!({
            "receipt_version": "shipper.receipt.v1",
            "plan_id": "test-plan",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T01:00:00Z",
            "packages": [],
            "event_log_path": ".shipper/events.jsonl",
            "git_context": {
                "commit": "abc123",
                "branch": "main",
                "tag": null,
                "dirty": false
            }
        });

        let receipt = migrate_v1_to_v2(v1_json).expect("migrate receipt");

        assert_eq!(receipt.receipt_version, CURRENT_RECEIPT_VERSION);
        assert!(receipt.git_context.is_some());
        let ctx = receipt.git_context.unwrap();
        assert_eq!(ctx.commit, Some("abc123".to_string()));
        assert_eq!(ctx.branch, Some("main".to_string()));
    }

    #[test]
    fn migrate_v1_to_v2_preserves_existing_environment() {
        let v1_json = serde_json::json!({
            "receipt_version": "shipper.receipt.v1",
            "plan_id": "test-plan",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T01:00:00Z",
            "packages": [],
            "event_log_path": ".shipper/events.jsonl",
            "environment": {
                "shipper_version": "0.1.0",
                "cargo_version": "1.75.0",
                "rust_version": "1.75.0",
                "os": "linux",
                "arch": "x86_64"
            }
        });

        let receipt = migrate_v1_to_v2(v1_json).expect("migrate receipt");

        assert_eq!(receipt.receipt_version, CURRENT_RECEIPT_VERSION);
        assert_eq!(receipt.environment.shipper_version, "0.1.0");
        assert_eq!(
            receipt.environment.cargo_version,
            Some("1.75.0".to_string())
        );
    }

    #[test]
    fn load_receipt_handles_missing_receipt_version_field() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        let path = receipt_path(&dir);

        // Create a receipt without receipt_version field (should default to v1)
        let receipt_json = r#"{
            "plan_id": "test-plan",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T01:00:00Z",
            "packages": [],
            "event_log_path": ".shipper/events.jsonl"
        }"#;

        fs::write(&path, receipt_json).expect("write receipt");

        let receipt = load_receipt(&dir)
            .expect("load receipt")
            .expect("receipt exists");

        // Should be migrated to v2
        assert_eq!(receipt.receipt_version, CURRENT_RECEIPT_VERSION);
    }

    #[test]
    fn load_receipt_handles_future_version() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        let path = receipt_path(&dir);

        // Create a receipt with a future version (should still load if format is compatible)
        let receipt_json = r#"{
            "receipt_version": "shipper.receipt.v99",
            "plan_id": "test-plan",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "started_at": "2024-01-01T00:00:00Z",
            "finished_at": "2024-01-01T01:00:00Z",
            "packages": [],
            "event_log_path": ".shipper/events.jsonl",
            "git_context": null,
            "environment": {
                "shipper_version": "0.1.0",
                "cargo_version": null,
                "rust_version": null,
                "os": "linux",
                "arch": "x86_64"
            }
        }"#;

        fs::write(&path, receipt_json).expect("write receipt");

        // Future versions are accepted if above minimum supported
        let receipt = load_receipt(&dir)
            .expect("load receipt")
            .expect("receipt exists");
        assert_eq!(receipt.receipt_version, "shipper.receipt.v99");
    }

    #[test]
    fn has_incomplete_state_returns_true_when_state_exists_without_receipt() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        // Create state file but not receipt
        let st = sample_state();
        save_state(&dir, &st).expect("save state");

        assert!(has_incomplete_state(&dir));
    }

    #[test]
    fn has_incomplete_state_returns_false_when_receipt_exists() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        // Create both state and receipt
        let st = sample_state();
        save_state(&dir, &st).expect("save state");
        write_receipt(&dir, &sample_receipt()).expect("write receipt");

        assert!(!has_incomplete_state(&dir));
    }

    #[test]
    fn has_incomplete_state_returns_false_when_no_state_exists() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        assert!(!has_incomplete_state(&dir));
    }

    #[test]
    fn clear_state_removes_state_file() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        // Create state file
        let st = sample_state();
        save_state(&dir, &st).expect("save state");
        assert!(state_path(&dir).exists());

        // Clear state
        clear_state(&dir).expect("clear state");
        assert!(!state_path(&dir).exists());
    }

    #[test]
    fn clear_state_does_not_remove_receipt_file() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("out");
        fs::create_dir_all(&dir).expect("mkdir");

        // Create both state and receipt
        let st = sample_state();
        save_state(&dir, &st).expect("save state");
        write_receipt(&dir, &sample_receipt()).expect("write receipt");

        // Clear state only
        clear_state(&dir).expect("clear state");
        assert!(!state_path(&dir).exists());
        assert!(receipt_path(&dir).exists());
    }
}
