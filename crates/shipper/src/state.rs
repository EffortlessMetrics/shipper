use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::types::{ExecutionState, Receipt};

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

/// Load receipt from state directory
pub fn load_receipt(state_dir: &Path) -> Result<Option<Receipt>> {
    let path = receipt_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read receipt file {}", path.display()))?;
    let receipt: Receipt = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse receipt JSON {}", path.display()))?;
    Ok(Some(receipt))
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
        assert!(content.contains("\"shipper.receipt.v1\""));
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
}
