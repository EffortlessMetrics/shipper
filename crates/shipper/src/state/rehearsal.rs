//! Persisted rehearsal receipt (#97 PR 3).
//!
//! A `rehearsal.json` sidecar, scoped to a state dir, captures the outcome
//! of the most recent `shipper rehearse` run. The hard gate in
//! `engine::run_publish` reads this file to decide whether live dispatch
//! is authorized for the current plan_id.
//!
//! Why a sidecar and not just scanning `events.jsonl`?
//! - Fast lookup (O(1) file read vs O(N) log scan).
//! - Small, human-readable, easy to inspect ad-hoc.
//! - Append-only events are preserved unchanged; this file is a
//!   *projection*, same relationship as `state.json` to events.
//!
//! Write semantics mirror `state.json`: atomic write via `.tmp` + rename,
//! so a crash mid-write can't corrupt the file.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Canonical rehearsal receipt file name.
pub const REHEARSAL_FILE: &str = "rehearsal.json";

/// Current rehearsal receipt schema version.
pub const CURRENT_REHEARSAL_VERSION: &str = "shipper.rehearsal.v1";

/// Persisted outcome of a single rehearsal run.
///
/// The hard gate keys on `plan_id`: a rehearsal is considered fresh for
/// the current publish only if `plan_id` matches the workspace's current
/// plan id. If the workspace changes between rehearse and publish, the
/// plan_id flips and the rehearsal is (correctly) rejected as stale.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RehearsalReceipt {
    /// Schema version for forward compatibility. Readers ignore unknown
    /// higher versions best-effort, same as other schema-versioned files.
    pub schema_version: String,
    /// Plan ID the rehearsal ran against.
    pub plan_id: String,
    /// Name of the registry the rehearsal published to.
    pub registry: String,
    /// Whether the rehearsal passed end-to-end.
    pub passed: bool,
    pub packages_attempted: usize,
    pub packages_published: usize,
    /// Human-readable one-line summary (matches the CLI's stdout line).
    pub summary: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

pub fn rehearsal_path(state_dir: &Path) -> PathBuf {
    state_dir.join(REHEARSAL_FILE)
}

/// Atomic write: serialize → write to `.tmp` → rename. Crash-safe.
pub fn save_rehearsal(state_dir: &Path, receipt: &RehearsalReceipt) -> Result<()> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;
    let path = rehearsal_path(state_dir);
    let tmp = path.with_extension("json.tmp");
    let data =
        serde_json::to_vec_pretty(receipt).context("failed to serialize RehearsalReceipt")?;
    {
        let mut f =
            File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
        f.write_all(&data)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("failed to fsync {}", tmp.display()))?;
    }
    fs::rename(&tmp, &path)
        .with_context(|| format!("failed to rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Load the rehearsal receipt, if any. `Ok(None)` when the file doesn't
/// exist — that's the common "no rehearsal run yet" case, not an error.
pub fn load_rehearsal(state_dir: &Path) -> Result<Option<RehearsalReceipt>> {
    let path = rehearsal_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }
    let mut s = String::new();
    File::open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?
        .read_to_string(&mut s)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let receipt: RehearsalReceipt =
        serde_json::from_str(&s).with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample() -> RehearsalReceipt {
        RehearsalReceipt {
            schema_version: CURRENT_REHEARSAL_VERSION.to_string(),
            plan_id: "abc123".to_string(),
            registry: "rehearsal".to_string(),
            passed: true,
            packages_attempted: 3,
            packages_published: 3,
            summary: "rehearsed 3 packages successfully".to_string(),
            started_at: Utc::now(),
            completed_at: Utc::now(),
        }
    }

    #[test]
    fn save_then_load_roundtrip() {
        let td = tempdir().expect("tempdir");
        let receipt = sample();
        save_rehearsal(td.path(), &receipt).expect("save");
        let loaded = load_rehearsal(td.path()).expect("load").expect("some");
        assert_eq!(loaded, receipt);
    }

    #[test]
    fn load_missing_file_returns_none() {
        let td = tempdir().expect("tempdir");
        assert!(load_rehearsal(td.path()).expect("load").is_none());
    }

    #[test]
    fn save_overwrites_existing() {
        let td = tempdir().expect("tempdir");
        let r1 = sample();
        save_rehearsal(td.path(), &r1).expect("save 1");
        let mut r2 = r1.clone();
        r2.passed = false;
        r2.summary = "rehearsal failed".into();
        save_rehearsal(td.path(), &r2).expect("save 2");
        let loaded = load_rehearsal(td.path()).expect("load").expect("some");
        assert_eq!(loaded, r2);
    }

    #[test]
    fn rehearsal_path_is_under_state_dir() {
        let p = rehearsal_path(Path::new("/tmp/x"));
        assert_eq!(p, Path::new("/tmp/x").join(REHEARSAL_FILE));
    }
}
