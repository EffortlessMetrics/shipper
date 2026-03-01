use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use shipper_environment::collect_environment_fingerprint;
use shipper_types::{ExecutionState, Receipt};

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
    shipper_schema::validate_schema_version(version, MINIMUM_SUPPORTED_VERSION, "receipt")
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use shipper_types::{
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
        let result =
            shipper_schema::parse_schema_version("shipper.receipt.v2").expect("should parse");
        assert_eq!(result, 2);
    }

    #[test]
    fn parse_schema_version_handles_single_digit() {
        let result =
            shipper_schema::parse_schema_version("shipper.receipt.v1").expect("should parse");
        assert_eq!(result, 1);
    }

    #[test]
    fn parse_schema_version_handles_large_version() {
        let result =
            shipper_schema::parse_schema_version("shipper.receipt.v100").expect("should parse");
        assert_eq!(result, 100);
    }

    #[test]
    fn parse_schema_version_rejects_invalid_format() {
        let result = shipper_schema::parse_schema_version("invalid");
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
        let result = shipper_schema::parse_schema_version("receipt.v2");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_rejects_invalid_format_no_version() {
        let result = shipper_schema::parse_schema_version("shipper.receipt");
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_version_rejects_invalid_format_missing_v() {
        let result = shipper_schema::parse_schema_version("shipper.receipt.2");
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

    // ── Persistence double-roundtrip ────────────────────────────────

    #[test]
    fn state_double_save_produces_identical_json() {
        let td = tempdir().expect("tempdir");
        let dir1 = td.path().join("first");
        let dir2 = td.path().join("second");
        let st = sample_state();

        save_state(&dir1, &st).expect("first save");
        let loaded = load_state(&dir1).expect("load").expect("exists");
        save_state(&dir2, &loaded).expect("second save");

        let json1 = fs::read_to_string(state_path(&dir1)).expect("read first");
        let json2 = fs::read_to_string(state_path(&dir2)).expect("read second");
        assert_eq!(json1, json2, "save→load→save must produce identical JSON");
    }

    #[test]
    fn receipt_double_save_produces_identical_json() {
        let td = tempdir().expect("tempdir");
        let dir1 = td.path().join("first");
        let dir2 = td.path().join("second");
        let receipt = sample_receipt();

        write_receipt(&dir1, &receipt).expect("first write");
        let loaded = load_receipt(&dir1).expect("load").expect("exists");
        write_receipt(&dir2, &loaded).expect("second write");

        let json1 = fs::read_to_string(receipt_path(&dir1)).expect("read first");
        let json2 = fs::read_to_string(receipt_path(&dir2)).expect("read second");
        assert_eq!(json1, json2, "write→load→write must produce identical JSON");
    }

    // ── State lifecycle transitions ─────────────────────────────────

    #[test]
    fn state_lifecycle_pending_uploaded_published() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("lifecycle");

        let mut packages = BTreeMap::new();
        packages.insert(
            "crate-a@1.0.0".to_string(),
            PackageProgress {
                name: "crate-a".to_string(),
                version: "1.0.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: Utc::now(),
            },
        );

        let mut state = ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "lifecycle-plan".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };

        // Pending
        save_state(&dir, &state).expect("save pending");
        let loaded = load_state(&dir).expect("load").expect("exists");
        assert!(matches!(
            loaded.packages["crate-a@1.0.0"].state,
            PackageState::Pending
        ));

        // Pending → Uploaded
        state.packages.get_mut("crate-a@1.0.0").unwrap().state = PackageState::Uploaded;
        state.packages.get_mut("crate-a@1.0.0").unwrap().attempts = 1;
        save_state(&dir, &state).expect("save uploaded");
        let loaded = load_state(&dir).expect("load").expect("exists");
        assert!(matches!(
            loaded.packages["crate-a@1.0.0"].state,
            PackageState::Uploaded
        ));

        // Uploaded → Published
        state.packages.get_mut("crate-a@1.0.0").unwrap().state = PackageState::Published;
        save_state(&dir, &state).expect("save published");
        let loaded = load_state(&dir).expect("load").expect("exists");
        assert!(matches!(
            loaded.packages["crate-a@1.0.0"].state,
            PackageState::Published
        ));
        assert_eq!(loaded.packages["crate-a@1.0.0"].attempts, 1);
    }

    #[test]
    fn state_all_error_classes_persist() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("errors");

        let mut packages = BTreeMap::new();
        for (key, class, msg) in [
            ("a@1.0.0", shipper_types::ErrorClass::Retryable, "timeout"),
            ("b@1.0.0", shipper_types::ErrorClass::Permanent, "denied"),
            ("c@1.0.0", shipper_types::ErrorClass::Ambiguous, "unclear"),
        ] {
            let name = key.split('@').next().unwrap();
            packages.insert(
                key.to_string(),
                PackageProgress {
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Failed {
                        class,
                        message: msg.to_string(),
                    },
                    last_updated_at: Utc::now(),
                },
            );
        }

        let state = ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "error-plan".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };

        save_state(&dir, &state).expect("save");
        let loaded = load_state(&dir).expect("load").expect("exists");
        assert_eq!(loaded.packages.len(), 3);

        match &loaded.packages["a@1.0.0"].state {
            PackageState::Failed { class, .. } => {
                assert!(matches!(class, shipper_types::ErrorClass::Retryable));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        match &loaded.packages["b@1.0.0"].state {
            PackageState::Failed { class, .. } => {
                assert!(matches!(class, shipper_types::ErrorClass::Permanent));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        match &loaded.packages["c@1.0.0"].state {
            PackageState::Failed { class, .. } => {
                assert!(matches!(class, shipper_types::ErrorClass::Ambiguous));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // ── Edge cases ──────────────────────────────────────────────────

    #[test]
    fn state_empty_packages_roundtrip() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("empty");

        let state = ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "empty-plan".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };

        save_state(&dir, &state).expect("save");
        let loaded = load_state(&dir).expect("load").expect("exists");
        assert!(loaded.packages.is_empty());
        assert_eq!(loaded.plan_id, "empty-plan");
    }

    #[test]
    fn receipt_empty_packages_roundtrip() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("empty");

        let empty_receipt = Receipt {
            packages: vec![],
            ..sample_receipt()
        };

        write_receipt(&dir, &empty_receipt).expect("write");
        let loaded = load_receipt(&dir).expect("load").expect("exists");
        assert!(loaded.packages.is_empty());
    }

    #[test]
    fn clear_state_noop_when_no_state_exists() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("noop");
        fs::create_dir_all(&dir).expect("mkdir");
        clear_state(&dir).expect("clear on empty dir");
        assert!(!state_path(&dir).exists());
    }

    #[test]
    fn load_receipt_returns_none_when_missing() {
        let td = tempdir().expect("tempdir");
        let loaded = load_receipt(td.path()).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn state_overwrite_replaces_all_data() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("overwrite");

        let st1 = ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-v1".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };
        save_state(&dir, &st1).expect("save v1");

        let mut packages = BTreeMap::new();
        packages.insert(
            "new@2.0.0".to_string(),
            PackageProgress {
                name: "new".to_string(),
                version: "2.0.0".to_string(),
                attempts: 3,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );
        let st2 = ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-v2".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };
        save_state(&dir, &st2).expect("save v2");

        let loaded = load_state(&dir).expect("load").expect("exists");
        assert_eq!(loaded.plan_id, "plan-v2");
        assert_eq!(loaded.packages.len(), 1);
        assert!(loaded.packages.contains_key("new@2.0.0"));
    }

    #[test]
    fn state_version_constant_preserved_on_roundtrip() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("version");
        save_state(&dir, &sample_state()).expect("save");
        let loaded = load_state(&dir).expect("load").expect("exists");
        assert_eq!(loaded.state_version, CURRENT_STATE_VERSION);
    }

    #[test]
    fn receipt_version_constant_preserved_on_roundtrip() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("version");
        write_receipt(&dir, &sample_receipt()).expect("write");
        let loaded = load_receipt(&dir).expect("load").expect("exists");
        assert_eq!(loaded.receipt_version, CURRENT_RECEIPT_VERSION);
    }

    #[test]
    fn state_with_special_chars_in_plan_id() {
        let td = tempdir().expect("tempdir");
        let special_ids = [
            "plan with spaces",
            "plan/with/slashes",
            "plan\"with\"quotes",
            "план-юникод",
            "\u{1f680}release-v1",
        ];

        for (i, plan_id) in special_ids.iter().enumerate() {
            let dir = td.path().join(format!("special-{i}"));
            let state = ExecutionState {
                state_version: CURRENT_STATE_VERSION.to_string(),
                plan_id: plan_id.to_string(),
                registry: Registry::crates_io(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                packages: BTreeMap::new(),
            };

            save_state(&dir, &state).expect("save");
            let loaded = load_state(&dir).expect("load").expect("exists");
            assert_eq!(loaded.plan_id, *plan_id);
        }
    }

    #[test]
    fn state_plan_id_mismatch_detection() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("mismatch");

        let state = ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "original-plan-abc".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages: BTreeMap::new(),
        };
        save_state(&dir, &state).expect("save");

        let loaded = load_state(&dir).expect("load").expect("exists");
        assert_ne!(loaded.plan_id, "different-plan-xyz");
        assert_eq!(loaded.plan_id, "original-plan-abc");
    }

    #[test]
    fn receipt_with_high_attempt_count() {
        let td = tempdir().expect("tempdir");
        let dir = td.path().join("high-attempts");

        let receipt = Receipt {
            packages: vec![PackageReceipt {
                name: "flaky".to_string(),
                version: "1.0.0".to_string(),
                attempts: 99,
                state: PackageState::Published,
                started_at: Utc::now(),
                finished_at: Utc::now(),
                duration_ms: 999_999,
                evidence: shipper_types::PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }],
            ..sample_receipt()
        };

        write_receipt(&dir, &receipt).expect("write");
        let loaded = load_receipt(&dir).expect("load").expect("exists");
        assert_eq!(loaded.packages[0].attempts, 99);
        assert_eq!(loaded.packages[0].duration_ms, 999_999);
    }

    // ── Insta snapshot helpers ──────────────────────────────────────

    use chrono::TimeZone;

    /// Build an `ExecutionState` with deterministic, fixed timestamps so
    /// that snapshot output is stable across runs.
    fn deterministic_state() -> ExecutionState {
        let fixed = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
        let mut packages = BTreeMap::new();
        packages.insert(
            "alpha@0.1.0".to_string(),
            PackageProgress {
                name: "alpha".to_string(),
                version: "0.1.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: fixed,
            },
        );
        packages.insert(
            "beta@0.2.0".to_string(),
            PackageProgress {
                name: "beta".to_string(),
                version: "0.2.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: fixed,
            },
        );

        ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "plan-abc123".to_string(),
            registry: Registry::crates_io(),
            created_at: fixed,
            updated_at: fixed,
            packages,
        }
    }

    /// Build a `Receipt` with deterministic timestamps.
    fn deterministic_receipt() -> Receipt {
        let fixed = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
        let finished = Utc.with_ymd_and_hms(2025, 1, 15, 12, 5, 0).unwrap();

        Receipt {
            receipt_version: CURRENT_RECEIPT_VERSION.to_string(),
            plan_id: "plan-abc123".to_string(),
            registry: Registry::crates_io(),
            started_at: fixed,
            finished_at: finished,
            packages: vec![
                PackageReceipt {
                    name: "alpha".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: fixed,
                    finished_at: finished,
                    duration_ms: 300_000,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "beta".to_string(),
                    version: "0.2.0".to_string(),
                    attempts: 2,
                    state: PackageState::Failed {
                        class: shipper_types::ErrorClass::Retryable,
                        message: "registry timeout".to_string(),
                    },
                    started_at: fixed,
                    finished_at: finished,
                    duration_ms: 120_000,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: shipper_types::EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: Some("1.82.0".to_string()),
                rust_version: Some("1.82.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        }
    }

    // ── Snapshot: state JSON format ─────────────────────────────────

    #[test]
    fn snapshot_state_json_format() {
        let state = deterministic_state();
        let json = serde_json::to_string_pretty(&state).expect("serialize state");
        insta::assert_snapshot!("state_json_format", json);
    }

    // ── Snapshot: receipt JSON format ────────────────────────────────

    #[test]
    fn snapshot_receipt_json_format() {
        let receipt = deterministic_receipt();
        let json = serde_json::to_string_pretty(&receipt).expect("serialize receipt");
        insta::assert_snapshot!("receipt_json_format", json);
    }

    // ── Snapshot: state transitions ─────────────────────────────────

    #[test]
    fn snapshot_state_transition_pending_to_published() {
        let fixed = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
        let mut state = deterministic_state();

        // Transition alpha from Pending → Published
        if let Some(pkg) = state.packages.get_mut("alpha@0.1.0") {
            pkg.state = PackageState::Published;
            pkg.attempts = 1;
            pkg.last_updated_at = fixed;
        }
        state.updated_at = fixed;

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("state_transition_pending_to_published", json);
    }

    #[test]
    fn snapshot_state_transition_pending_to_failed() {
        let fixed = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
        let mut state = deterministic_state();

        // Transition alpha from Pending → Failed
        if let Some(pkg) = state.packages.get_mut("alpha@0.1.0") {
            pkg.state = PackageState::Failed {
                class: shipper_types::ErrorClass::Permanent,
                message: "crate name is reserved".to_string(),
            };
            pkg.attempts = 1;
            pkg.last_updated_at = fixed;
        }
        state.updated_at = fixed;

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("state_transition_pending_to_failed", json);
    }

    #[test]
    fn snapshot_state_transition_pending_to_skipped() {
        let fixed = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
        let mut state = deterministic_state();

        // Transition alpha from Pending → Skipped
        if let Some(pkg) = state.packages.get_mut("alpha@0.1.0") {
            pkg.state = PackageState::Skipped {
                reason: "already published".to_string(),
            };
            pkg.attempts = 0;
            pkg.last_updated_at = fixed;
        }
        state.updated_at = fixed;

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("state_transition_pending_to_skipped", json);
    }

    // ── Snapshot: state with all PackageState variants ─────────────

    #[test]
    fn snapshot_state_all_lifecycle_variants() {
        let fixed = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
        let mut packages = BTreeMap::new();
        packages.insert(
            "crate-a@1.0.0".to_string(),
            PackageProgress {
                name: "crate-a".to_string(),
                version: "1.0.0".to_string(),
                attempts: 0,
                state: PackageState::Pending,
                last_updated_at: fixed,
            },
        );
        packages.insert(
            "crate-b@1.0.0".to_string(),
            PackageProgress {
                name: "crate-b".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Uploaded,
                last_updated_at: fixed,
            },
        );
        packages.insert(
            "crate-c@1.0.0".to_string(),
            PackageProgress {
                name: "crate-c".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Published,
                last_updated_at: fixed,
            },
        );
        packages.insert(
            "crate-d@1.0.0".to_string(),
            PackageProgress {
                name: "crate-d".to_string(),
                version: "1.0.0".to_string(),
                attempts: 0,
                state: PackageState::Skipped {
                    reason: "already published".to_string(),
                },
                last_updated_at: fixed,
            },
        );
        packages.insert(
            "crate-e@1.0.0".to_string(),
            PackageProgress {
                name: "crate-e".to_string(),
                version: "1.0.0".to_string(),
                attempts: 3,
                state: PackageState::Failed {
                    class: shipper_types::ErrorClass::Retryable,
                    message: "network timeout".to_string(),
                },
                last_updated_at: fixed,
            },
        );
        packages.insert(
            "crate-f@1.0.0".to_string(),
            PackageProgress {
                name: "crate-f".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: PackageState::Ambiguous {
                    message: "upload status unknown".to_string(),
                },
                last_updated_at: fixed,
            },
        );

        let state = ExecutionState {
            state_version: CURRENT_STATE_VERSION.to_string(),
            plan_id: "all-variants-plan".to_string(),
            registry: Registry::crates_io(),
            created_at: fixed,
            updated_at: fixed,
            packages,
        };

        let json = serde_json::to_string_pretty(&state).expect("serialize");
        insta::assert_snapshot!("state_all_lifecycle_variants", json);
    }

    // ── Snapshot: receipt with all packages failed ───────────────────

    #[test]
    fn snapshot_receipt_all_failed() {
        let fixed = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
        let finished = Utc.with_ymd_and_hms(2025, 1, 15, 12, 10, 0).unwrap();

        let receipt = Receipt {
            receipt_version: CURRENT_RECEIPT_VERSION.to_string(),
            plan_id: "all-failed-plan".to_string(),
            registry: Registry::crates_io(),
            started_at: fixed,
            finished_at: finished,
            packages: vec![
                PackageReceipt {
                    name: "core".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 3,
                    state: PackageState::Failed {
                        class: shipper_types::ErrorClass::Retryable,
                        message: "registry timeout after 3 attempts".to_string(),
                    },
                    started_at: fixed,
                    finished_at: finished,
                    duration_ms: 180_000,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
                PackageReceipt {
                    name: "utils".to_string(),
                    version: "0.5.0".to_string(),
                    attempts: 1,
                    state: PackageState::Failed {
                        class: shipper_types::ErrorClass::Permanent,
                        message: "crate name is reserved".to_string(),
                    },
                    started_at: fixed,
                    finished_at: finished,
                    duration_ms: 5_000,
                    evidence: shipper_types::PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                },
            ],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: shipper_types::EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: Some("1.82.0".to_string()),
                rust_version: Some("1.82.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&receipt).expect("serialize");
        insta::assert_snapshot!("receipt_all_failed", json);
    }

    // ── Property-based tests (proptest) ─────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_error_class() -> impl Strategy<Value = shipper_types::ErrorClass> {
            prop_oneof![
                Just(shipper_types::ErrorClass::Retryable),
                Just(shipper_types::ErrorClass::Permanent),
                Just(shipper_types::ErrorClass::Ambiguous),
            ]
        }

        fn arb_package_state() -> impl Strategy<Value = PackageState> {
            prop_oneof![
                Just(PackageState::Pending),
                Just(PackageState::Uploaded),
                Just(PackageState::Published),
                "\\PC{1,50}".prop_map(|reason| PackageState::Skipped { reason }),
                (arb_error_class(), "\\PC{1,50}")
                    .prop_map(|(class, message)| PackageState::Failed { class, message }),
                "\\PC{1,50}".prop_map(|message| PackageState::Ambiguous { message }),
            ]
        }

        fn arb_registry() -> impl Strategy<Value = Registry> {
            (
                "[a-z][a-z0-9-]{0,19}",
                "https?://[a-z]{1,10}\\.[a-z]{2,4}",
                proptest::option::of("https?://[a-z]{1,10}\\.[a-z]{2,4}"),
            )
                .prop_map(|(name, api_base, index_base)| Registry {
                    name,
                    api_base,
                    index_base,
                })
        }

        fn arb_datetime() -> impl Strategy<Value = chrono::DateTime<Utc>> {
            (0i64..=4_000_000_000i64)
                .prop_map(|secs| chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default())
        }

        fn arb_package_progress() -> impl Strategy<Value = PackageProgress> {
            (
                "[a-z][a-z0-9_-]{0,19}",
                "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
                0u32..100,
                arb_package_state(),
                arb_datetime(),
            )
                .prop_map(|(name, version, attempts, state, ts)| PackageProgress {
                    name,
                    version,
                    attempts,
                    state,
                    last_updated_at: ts,
                })
        }

        fn arb_execution_state() -> impl Strategy<Value = ExecutionState> {
            (
                arb_registry(),
                arb_datetime(),
                arb_datetime(),
                proptest::collection::btree_map(
                    "[a-z]{1,8}@[0-9]{1,2}\\.[0-9]{1,2}\\.[0-9]{1,2}",
                    arb_package_progress(),
                    0..5,
                ),
                "\\PC{1,30}",
            )
                .prop_map(|(registry, created, updated, packages, plan_id)| {
                    ExecutionState {
                        state_version: CURRENT_STATE_VERSION.to_string(),
                        plan_id,
                        registry,
                        created_at: created,
                        updated_at: updated,
                        packages,
                    }
                })
        }

        fn arb_receipt() -> impl Strategy<Value = Receipt> {
            (
                arb_registry(),
                arb_datetime(),
                arb_datetime(),
                proptest::collection::vec(arb_package_receipt(), 0..5),
                "\\PC{1,30}",
            )
                .prop_map(|(registry, started, finished, packages, plan_id)| Receipt {
                    receipt_version: CURRENT_RECEIPT_VERSION.to_string(),
                    plan_id,
                    registry,
                    started_at: started,
                    finished_at: finished,
                    packages,
                    event_log_path: PathBuf::from(".shipper/events.jsonl"),
                    git_context: None,
                    environment: shipper_types::EnvironmentFingerprint {
                        shipper_version: "0.1.0".to_string(),
                        cargo_version: Some("1.80.0".to_string()),
                        rust_version: Some("1.80.0".to_string()),
                        os: "linux".to_string(),
                        arch: "x86_64".to_string(),
                    },
                })
        }

        fn arb_package_receipt() -> impl Strategy<Value = PackageReceipt> {
            (
                "[a-z][a-z0-9_-]{0,19}",
                "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
                0u32..100,
                arb_package_state(),
                arb_datetime(),
                arb_datetime(),
                0u128..1_000_000,
            )
                .prop_map(
                    |(name, version, attempts, state, started, finished, dur)| PackageReceipt {
                        name,
                        version,
                        attempts,
                        state,
                        started_at: started,
                        finished_at: finished,
                        duration_ms: dur,
                        evidence: shipper_types::PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                    },
                )
        }

        // ── State serialization roundtrip ───────────────────────────

        proptest! {
            #[test]
            fn execution_state_json_roundtrip(state in arb_execution_state()) {
                let json = serde_json::to_string(&state).expect("serialize");
                let deser: ExecutionState = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(state.plan_id, deser.plan_id);
                prop_assert_eq!(state.state_version, deser.state_version);
                prop_assert_eq!(state.registry.name, deser.registry.name);
                prop_assert_eq!(state.packages.len(), deser.packages.len());
                for (k, v) in &state.packages {
                    let d = deser.packages.get(k).expect("key exists");
                    prop_assert_eq!(&v.name, &d.name);
                    prop_assert_eq!(&v.version, &d.version);
                    prop_assert_eq!(v.attempts, d.attempts);
                }
            }

            #[test]
            fn receipt_json_roundtrip(receipt in arb_receipt()) {
                let json = serde_json::to_string(&receipt).expect("serialize");
                let deser: Receipt = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(receipt.plan_id, deser.plan_id);
                prop_assert_eq!(receipt.receipt_version, deser.receipt_version);
                prop_assert_eq!(receipt.registry.name, deser.registry.name);
                prop_assert_eq!(receipt.packages.len(), deser.packages.len());
            }
        }

        // ── Plan ID with arbitrary inputs ───────────────────────────

        proptest! {
            #[test]
            fn plan_id_survives_roundtrip(plan_id in "\\PC{1,64}") {
                let fixed = chrono::DateTime::from_timestamp(1_700_000_000, 0)
                    .unwrap_or_default();
                let state = ExecutionState {
                    state_version: CURRENT_STATE_VERSION.to_string(),
                    plan_id: plan_id.clone(),
                    registry: Registry::crates_io(),
                    created_at: fixed,
                    updated_at: fixed,
                    packages: BTreeMap::new(),
                };
                let json = serde_json::to_string(&state).expect("serialize");
                let deser: ExecutionState = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(plan_id, deser.plan_id);
            }
        }

        // ── Package state transitions ───────────────────────────────

        proptest! {
            #[test]
            fn package_state_roundtrips_through_json(pkg_state in arb_package_state()) {
                let json = serde_json::to_string(&pkg_state).expect("serialize");
                let deser: PackageState = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(pkg_state, deser);
            }

            #[test]
            fn package_progress_state_update_persists(
                initial in arb_package_progress(),
                new_state in arb_package_state(),
            ) {
                let fixed = chrono::DateTime::from_timestamp(1_700_000_000, 0)
                    .unwrap_or_default();
                let mut packages = BTreeMap::new();
                let key = format!("{}@{}", initial.name, initial.version);
                packages.insert(key.clone(), initial);

                let mut state = ExecutionState {
                    state_version: CURRENT_STATE_VERSION.to_string(),
                    plan_id: "test".to_string(),
                    registry: Registry::crates_io(),
                    created_at: fixed,
                    updated_at: fixed,
                    packages,
                };

                // Apply state transition
                if let Some(pkg) = state.packages.get_mut(&key) {
                    pkg.state = new_state.clone();
                    pkg.last_updated_at = fixed;
                }

                let json = serde_json::to_string(&state).expect("serialize");
                let deser: ExecutionState = serde_json::from_str(&json).expect("deserialize");
                let pkg = deser.packages.get(&key).expect("package exists");
                prop_assert_eq!(&new_state, &pkg.state);
            }
        }

        // ── Atomic write/read consistency ───────────────────────────

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(20))]

            #[test]
            fn save_load_state_is_consistent(state in arb_execution_state()) {
                let td = tempfile::tempdir().expect("tempdir");
                let dir = td.path().join("proptest-state");

                save_state(&dir, &state).expect("save");
                let loaded = load_state(&dir).expect("load").expect("exists");

                prop_assert_eq!(state.plan_id, loaded.plan_id);
                prop_assert_eq!(state.state_version, loaded.state_version);
                prop_assert_eq!(state.registry.name, loaded.registry.name);
                prop_assert_eq!(state.registry.api_base, loaded.registry.api_base);
                prop_assert_eq!(state.packages.len(), loaded.packages.len());
                for (k, v) in &state.packages {
                    let d = loaded.packages.get(k).expect("key exists");
                    prop_assert_eq!(&v.name, &d.name);
                    prop_assert_eq!(&v.version, &d.version);
                    prop_assert_eq!(v.attempts, d.attempts);
                }
            }

            #[test]
            fn save_load_receipt_is_consistent(receipt in arb_receipt()) {
                let td = tempfile::tempdir().expect("tempdir");
                let dir = td.path().join("proptest-receipt");

                write_receipt(&dir, &receipt).expect("write");
                let loaded = load_receipt(&dir).expect("load").expect("exists");

                prop_assert_eq!(receipt.plan_id, loaded.plan_id);
                prop_assert_eq!(receipt.receipt_version, loaded.receipt_version);
                prop_assert_eq!(receipt.registry.name, loaded.registry.name);
                prop_assert_eq!(receipt.packages.len(), loaded.packages.len());
                for (orig, ld) in receipt.packages.iter().zip(loaded.packages.iter()) {
                    prop_assert_eq!(&orig.name, &ld.name);
                    prop_assert_eq!(&orig.version, &ld.version);
                    prop_assert_eq!(orig.attempts, ld.attempts);
                }
            }
        }

        // ── Double-roundtrip idempotency ────────────────────────────

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(20))]

            #[test]
            fn state_save_load_save_byte_identical(state in arb_execution_state()) {
                let td = tempfile::tempdir().expect("tempdir");
                let dir1 = td.path().join("first");
                let dir2 = td.path().join("second");

                save_state(&dir1, &state).expect("first save");
                let loaded = load_state(&dir1).expect("load").expect("exists");
                save_state(&dir2, &loaded).expect("second save");

                let json1 = fs::read_to_string(state_path(&dir1)).expect("read first");
                let json2 = fs::read_to_string(state_path(&dir2)).expect("read second");
                prop_assert_eq!(json1, json2);
            }

            #[test]
            fn receipt_save_load_save_byte_identical(receipt in arb_receipt()) {
                let td = tempfile::tempdir().expect("tempdir");
                let dir1 = td.path().join("first");
                let dir2 = td.path().join("second");

                write_receipt(&dir1, &receipt).expect("first write");
                let loaded = load_receipt(&dir1).expect("load").expect("exists");
                write_receipt(&dir2, &loaded).expect("second write");

                let json1 = fs::read_to_string(receipt_path(&dir1)).expect("read first");
                let json2 = fs::read_to_string(receipt_path(&dir2)).expect("read second");
                prop_assert_eq!(json1, json2);
            }
        }
    }
}
