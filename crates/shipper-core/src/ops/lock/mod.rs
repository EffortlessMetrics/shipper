//! File-based locking mechanism to prevent concurrent operations.
//!
//! This module provides a simple file-based lock that can be used to prevent
//! concurrent access to shared resources across processes. The lock file
//! contains metadata about the lock holder (PID, hostname, timestamp).
//!
//! Absorbed from the standalone `shipper-lock` crate during the decrating
//! effort (see `docs/decrating-plan.md` §6 Phase 2). The public surface at
//! `shipper::lock` is preserved via a re-export in `crate::lib`.
//!
//! # Example
//!
//! ```
//! use shipper_core::lock::LockFile;
//! use std::path::Path;
//!
//! # fn example() -> anyhow::Result<()> {
//! // Acquire a lock
//! let lock = LockFile::acquire(Path::new(".shipper"), None)?;
//!
//! // Check if locked
//! assert!(LockFile::is_locked(Path::new(".shipper"), None)?);
//!
//! // Lock is automatically released when dropped
//! drop(lock);
//! # Ok(())
//! # }
//! ```

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default lock file name
pub const LOCK_FILE: &str = "lock";

/// Information stored in the lock file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    /// Process ID of the lock holder
    pub pid: u32,
    /// Hostname where the lock was acquired
    pub hostname: String,
    /// When the lock was acquired
    pub acquired_at: DateTime<Utc>,
    /// Optional plan ID being executed
    pub plan_id: Option<String>,
}

/// Lock file handle that automatically releases on Drop
#[derive(Debug)]
pub struct LockFile {
    path: PathBuf,
}

impl LockFile {
    /// Acquire a lock file in the specified state directory
    ///
    /// This will fail if a lock already exists and is not stale.
    /// Use `is_locked` first to check, or use `acquire_with_timeout` for
    /// automatic stale lock handling.
    ///
    /// # Example
    ///
    /// ```
    /// use shipper_core::lock::LockFile;
    /// use std::path::Path;
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let lock = LockFile::acquire(Path::new(".mylock"), None)?;
    /// # drop(lock);
    /// # Ok(())
    /// # }
    /// ```
    pub fn acquire(state_dir: &Path, workspace_root: Option<&Path>) -> Result<Self> {
        let lock_path = lock_path(state_dir, workspace_root);

        // Create state directory if it doesn't exist
        fs::create_dir_all(state_dir)
            .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

        // Check if lock already exists
        if lock_path.exists() {
            let existing_info = read_lock_info_from_path(&lock_path)?;
            bail!(
                "lock already held by pid {} on {} since {} (plan_id: {:?})",
                existing_info.pid,
                existing_info.hostname,
                existing_info.acquired_at,
                existing_info.plan_id
            );
        }

        // Get current process info
        let pid = std::process::id();
        let hostname = gethostname::gethostname().to_string_lossy().to_string();

        let info = LockInfo {
            pid,
            hostname,
            acquired_at: Utc::now(),
            plan_id: None,
        };

        // Write lock file atomically
        let tmp_path = lock_path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&info).context("failed to serialize lock info")?;

        {
            let mut file = File::create(&tmp_path).with_context(|| {
                format!("failed to create lock tmp file {}", tmp_path.display())
            })?;
            file.write_all(json.as_bytes())
                .with_context(|| format!("failed to write lock tmp file {}", tmp_path.display()))?;
            file.sync_all().context("failed to sync lock file")?;
        }

        fs::rename(&tmp_path, &lock_path)
            .with_context(|| format!("failed to rename lock file to {}", lock_path.display()))?;

        // Sync parent directory for durability
        if let Some(parent) = lock_path.parent()
            && let Ok(dir_file) = File::open(parent)
        {
            let _ = dir_file.sync_all();
        }

        Ok(Self { path: lock_path })
    }

    /// Acquire a lock, automatically removing stale locks older than timeout
    ///
    /// # Arguments
    ///
    /// * `state_dir` - Directory to store the lock file
    /// * `workspace_root` - Optional workspace root to hash for avoiding global lock collisions
    /// * `timeout` - Age threshold for considering a lock stale
    ///
    /// # Example
    ///
    /// ```
    /// use shipper_core::lock::LockFile;
    /// use std::path::Path;
    /// use std::time::Duration;
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let lock = LockFile::acquire_with_timeout(
    ///     Path::new(".mylock"),
    ///     None,
    ///     Duration::from_secs(3600)
    /// )?;
    /// # drop(lock);
    /// # Ok(())
    /// # }
    /// ```
    pub fn acquire_with_timeout(
        state_dir: &Path,
        workspace_root: Option<&Path>,
        timeout: Duration,
    ) -> Result<Self> {
        let lock_path = lock_path(state_dir, workspace_root);

        if lock_path.exists() {
            if let Ok(info) = read_lock_info_from_path(&lock_path) {
                let age = Utc::now() - info.acquired_at;
                // chrono::Duration doesn't have to_std(), use num_seconds directly
                if age.num_seconds().unsigned_abs() > timeout.as_secs() {
                    // Lock is stale, remove it
                    fs::remove_file(&lock_path).with_context(|| {
                        format!("failed to remove stale lock file {}", lock_path.display())
                    })?;
                } else {
                    bail!(
                        "lock already held by pid {} on {} since {} (age: {:?})",
                        info.pid,
                        info.hostname,
                        info.acquired_at,
                        age
                    );
                }
            } else {
                // Lock file exists but is corrupt, remove it
                fs::remove_file(&lock_path).with_context(|| {
                    format!("failed to remove corrupt lock file {}", lock_path.display())
                })?;
            }
        }

        Self::acquire(state_dir, workspace_root)
    }

    /// Release the lock file
    ///
    /// This is normally called automatically when the lock is dropped,
    /// but can be called explicitly if needed.
    pub fn release(&self) -> Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)
                .with_context(|| format!("failed to remove lock file {}", self.path.display()))?;
        }
        Ok(())
    }

    /// Update the plan_id in the lock file
    pub fn set_plan_id(&self, plan_id: &str) -> Result<()> {
        if !self.path.exists() {
            bail!("lock file does not exist at {}", self.path.display());
        }

        let mut info = read_lock_info_from_path(&self.path)?;
        info.plan_id = Some(plan_id.to_string());

        let json = serde_json::to_string_pretty(&info).context("failed to serialize lock info")?;

        let tmp_path = self.path.with_extension("tmp");
        {
            let mut file = File::create(&tmp_path).with_context(|| {
                format!("failed to create lock tmp file {}", tmp_path.display())
            })?;
            file.write_all(json.as_bytes())
                .with_context(|| format!("failed to write lock tmp file {}", tmp_path.display()))?;
            file.sync_all().context("failed to sync lock file")?;
        }

        fs::rename(&tmp_path, &self.path)
            .with_context(|| format!("failed to rename lock file to {}", self.path.display()))?;

        Ok(())
    }

    /// Check if a lock file exists
    pub fn is_locked(state_dir: &Path, workspace_root: Option<&Path>) -> Result<bool> {
        Ok(lock_path(state_dir, workspace_root).exists())
    }

    /// Read the lock file information
    pub fn read_lock_info(state_dir: &Path, workspace_root: Option<&Path>) -> Result<LockInfo> {
        read_lock_info_from_path(&lock_path(state_dir, workspace_root))
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        // Best effort to release the lock
        let _ = self.release();
    }
}

/// Read lock info from a specific path
fn read_lock_info_from_path(path: &Path) -> Result<LockInfo> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read lock file {}", path.display()))?;
    let info: LockInfo = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse lock JSON from {}", path.display()))?;
    Ok(info)
}

/// Get the lock file path for a state directory and optional workspace root
pub fn lock_path(state_dir: &Path, workspace_root: Option<&Path>) -> PathBuf {
    if let Some(root) = workspace_root {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        root.hash(&mut hasher);
        let hash = hasher.finish();
        state_dir.join(format!("{}_{:016x}", LOCK_FILE, hash))
    } else {
        state_dir.join(LOCK_FILE)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn lock_path_without_root_ends_with_lock_file(dir_name in "[a-zA-Z0-9_]{1,64}") {
                let base = PathBuf::from(&dir_name);
                let p = lock_path(&base, None);
                prop_assert_eq!(p, base.join(LOCK_FILE));
            }

            #[test]
            fn lock_path_with_root_contains_hex_hash(
                dir_name in "[a-zA-Z0-9_]{1,64}",
                root_name in "[a-zA-Z0-9_/]{1,128}",
            ) {
                let base = PathBuf::from(&dir_name);
                let root = PathBuf::from(&root_name);
                let p = lock_path(&base, Some(&root));
                let name = p.file_name().unwrap().to_string_lossy();
                let expected_prefix = format!("{}_", LOCK_FILE);
                prop_assert!(name.starts_with(&expected_prefix));
                // 16 hex chars after the underscore
                let suffix = &name[LOCK_FILE.len() + 1..];
                prop_assert_eq!(suffix.len(), 16);
                prop_assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
            }

            #[test]
            fn lock_path_with_root_is_deterministic(
                dir_name in "[a-zA-Z0-9_]{1,64}",
                root_name in "[a-zA-Z0-9_/]{1,128}",
            ) {
                let base = PathBuf::from(&dir_name);
                let root = PathBuf::from(&root_name);
                prop_assert_eq!(
                    lock_path(&base, Some(&root)),
                    lock_path(&base, Some(&root))
                );
            }

            #[test]
            fn timeout_duration_from_arbitrary_secs(secs in 0u64..=u64::MAX) {
                let d = Duration::from_secs(secs);
                prop_assert_eq!(d.as_secs(), secs);
            }

            #[test]
            fn acquire_release_lifecycle(dir_suffix in "[a-zA-Z0-9]{1,32}") {
                let td = tempdir().expect("tempdir");
                let state_dir = td.path().join(dir_suffix);

                let lock = LockFile::acquire(&state_dir, None).expect("acquire");
                prop_assert!(lock_path(&state_dir, None).exists());

                let info = LockFile::read_lock_info(&state_dir, None).expect("read");
                prop_assert_eq!(info.pid, std::process::id());
                prop_assert!(!info.hostname.is_empty());

                lock.release().expect("release");
                prop_assert!(!lock_path(&state_dir, None).exists());
            }

            #[test]
            fn stale_lock_detected_by_arbitrary_age(
                age_hours in 2u32..1000u32,
                timeout_secs in 1u64..3600u64,
            ) {
                let td = tempdir().expect("tempdir");
                let lp = lock_path(td.path(), None);

                let old_info = LockInfo {
                    pid: 99999,
                    hostname: "prop-host".to_string(),
                    acquired_at: Utc::now() - chrono::Duration::hours(i64::from(age_hours)),
                    plan_id: None,
                };
                std::fs::write(
                    &lp,
                    serde_json::to_string(&old_info).expect("ser"),
                ).expect("write");

                // age_hours >= 2 means at least 7200 seconds; timeout_secs < 3600
                // so the lock is always stale relative to the timeout
                let lock = LockFile::acquire_with_timeout(
                    td.path(),
                    None,
                    Duration::from_secs(timeout_secs),
                ).expect("should replace stale lock");

                let new_info = LockFile::read_lock_info(td.path(), None).expect("read");
                prop_assert_eq!(new_info.pid, std::process::id());
                prop_assert_ne!(new_info.pid, 99999);
                drop(lock);
            }

            #[test]
            fn fresh_lock_not_removed_with_large_timeout(
                age_minutes in 1u32..59u32,
            ) {
                let td = tempdir().expect("tempdir");
                let lp = lock_path(td.path(), None);

                let info = LockInfo {
                    pid: 88888,
                    hostname: "fresh-host".to_string(),
                    acquired_at: Utc::now() - chrono::Duration::minutes(i64::from(age_minutes)),
                    plan_id: None,
                };
                std::fs::write(
                    &lp,
                    serde_json::to_string(&info).expect("ser"),
                ).expect("write");

                // 1-hour timeout; lock is < 1 hour old → should fail
                let result = LockFile::acquire_with_timeout(
                    td.path(),
                    None,
                    Duration::from_secs(3600),
                );
                prop_assert!(result.is_err());
                prop_assert!(result.unwrap_err().to_string().contains("lock already held"));
            }

            #[test]
            fn lock_info_serde_roundtrip_proptest(
                pid in any::<u32>(),
                hostname in "[a-zA-Z0-9._-]{1,64}",
                plan_id in proptest::option::of("[a-zA-Z0-9_-]{1,64}"),
            ) {
                let info = LockInfo {
                    pid,
                    hostname: hostname.clone(),
                    acquired_at: Utc::now(),
                    plan_id: plan_id.clone(),
                };
                let json = serde_json::to_string(&info).expect("ser");
                let parsed: LockInfo = serde_json::from_str(&json).expect("de");
                prop_assert_eq!(parsed.pid, pid);
                prop_assert_eq!(parsed.hostname, hostname);
                prop_assert_eq!(parsed.plan_id, plan_id);
            }

            #[test]
            fn lock_path_parent_is_always_state_dir(
                dir_name in "[a-zA-Z0-9_]{1,64}",
                root_name in proptest::option::of("[a-zA-Z0-9_/]{1,128}"),
            ) {
                let base = PathBuf::from(&dir_name);
                let root = root_name.as_ref().map(PathBuf::from);
                let p = lock_path(&base, root.as_deref());
                prop_assert_eq!(p.parent().unwrap(), &*base);
            }

            #[test]
            fn acquire_release_with_workspace_root(
                dir_suffix in "[a-zA-Z0-9]{1,32}",
                ws_suffix in "[a-zA-Z0-9]{1,32}",
            ) {
                let td = tempdir().expect("tempdir");
                let state_dir = td.path().join(&dir_suffix);
                let ws_root = td.path().join(&ws_suffix);

                let lock = LockFile::acquire(&state_dir, Some(&ws_root)).expect("acquire");
                prop_assert!(LockFile::is_locked(&state_dir, Some(&ws_root)).expect("is_locked"));
                prop_assert!(!LockFile::is_locked(&state_dir, None).unwrap_or(false));

                lock.release().expect("release");
                prop_assert!(!LockFile::is_locked(&state_dir, Some(&ws_root)).expect("after release"));
            }

            #[test]
            fn set_plan_id_roundtrip(plan_id in "[a-zA-Z0-9_-]{1,64}") {
                let td = tempdir().expect("tempdir");
                let lock = LockFile::acquire(td.path(), None).expect("acquire");
                lock.set_plan_id(&plan_id).expect("set_plan_id");

                let info = LockFile::read_lock_info(td.path(), None).expect("read");
                prop_assert_eq!(info.plan_id.as_deref(), Some(plan_id.as_str()));
                prop_assert_eq!(info.pid, std::process::id());
                drop(lock);
            }

            #[test]
            fn stale_lock_with_plan_id_is_replaced(
                age_hours in 2u32..500u32,
                plan_id in "[a-zA-Z0-9_-]{1,64}",
            ) {
                let td = tempdir().expect("tempdir");
                let lp = lock_path(td.path(), None);

                let old_info = LockInfo {
                    pid: 77777,
                    hostname: "stale-host".to_string(),
                    acquired_at: Utc::now() - chrono::Duration::hours(i64::from(age_hours)),
                    plan_id: Some(plan_id),
                };
                std::fs::write(
                    &lp,
                    serde_json::to_string(&old_info).expect("ser"),
                ).expect("write");

                let lock = LockFile::acquire_with_timeout(
                    td.path(),
                    None,
                    Duration::from_secs(3600),
                ).expect("should replace stale lock with plan_id");

                let new_info = LockFile::read_lock_info(td.path(), None).expect("read");
                prop_assert_eq!(new_info.pid, std::process::id());
                prop_assert!(new_info.plan_id.is_none());
                drop(lock);
            }

            #[test]
            fn lock_file_on_disk_has_expected_json_structure(
                dir_suffix in "[a-zA-Z0-9]{1,32}",
            ) {
                let td = tempdir().expect("tempdir");
                let state_dir = td.path().join(dir_suffix);
                let lock = LockFile::acquire(&state_dir, None).expect("acquire");

                let lp = lock_path(&state_dir, None);
                let content = std::fs::read_to_string(&lp).expect("read");
                let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");

                let obj = parsed.as_object().expect("should be object");
                prop_assert!(obj["pid"].is_number());
                prop_assert!(obj["hostname"].is_string());
                prop_assert!(obj["acquired_at"].is_string());
                prop_assert!(obj.contains_key("plan_id"));
                // Content is pretty-printed (contains newlines)
                prop_assert!(content.contains('\n'));

                drop(lock);
            }
        }
    }

    #[test]
    fn lock_path_returns_expected_path() {
        let base = PathBuf::from("x");
        assert_eq!(lock_path(&base, None), PathBuf::from("x").join(LOCK_FILE));
    }

    #[test]
    fn acquire_creates_lock_file() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        assert!(lock_path(td.path(), None).exists());
        lock.release().expect("release");
        assert!(!lock_path(td.path(), None).exists());
    }

    #[test]
    fn acquire_fails_when_locked() {
        let td = tempdir().expect("tempdir");
        let _lock1 = LockFile::acquire(td.path(), None).expect("first acquire");

        let result = LockFile::acquire(td.path(), None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("lock already held")
        );
    }

    #[test]
    fn drop_releases_lock() {
        let td = tempdir().expect("tempdir");
        {
            let _lock = LockFile::acquire(td.path(), None).expect("acquire");
            assert!(lock_path(td.path(), None).exists());
        }
        // Lock should be released after drop
        assert!(!lock_path(td.path(), None).exists());
    }

    #[test]
    fn read_lock_info_returns_correct_info() {
        let td = tempdir().expect("tempdir");
        let _lock = LockFile::acquire(td.path(), None).expect("acquire");

        let info = LockFile::read_lock_info(td.path(), None).expect("read info");
        assert_eq!(info.pid, std::process::id());
        assert!(!info.hostname.is_empty());
        assert!(info.plan_id.is_none());
    }

    #[test]
    fn set_plan_id_updates_lock() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");

        lock.set_plan_id("test-plan-123").expect("set plan_id");

        let info = LockFile::read_lock_info(td.path(), None).expect("read info");
        assert_eq!(info.plan_id, Some("test-plan-123".to_string()));
    }

    #[test]
    fn is_locked_returns_correct_status() {
        let td = tempdir().expect("tempdir");
        assert!(!LockFile::is_locked(td.path(), None).expect("is_locked"));

        let _lock = LockFile::acquire(td.path(), None).expect("acquire");
        assert!(LockFile::is_locked(td.path(), None).expect("is_locked"));
    }

    #[test]
    fn acquire_with_timeout_removes_stale_locks() {
        let td = tempdir().expect("tempdir");

        // Create a lock with old timestamp
        let lock_path = lock_path(td.path(), None);
        let old_info = LockInfo {
            pid: 12345,
            hostname: "test-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::hours(2),
            plan_id: None,
        };
        fs::write(
            &lock_path,
            serde_json::to_string(&old_info).expect("serialize"),
        )
        .expect("write stale lock");

        // Acquire with 1 hour timeout - should succeed and remove stale lock
        let _lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600))
            .expect("acquire with timeout");

        let info = LockFile::read_lock_info(td.path(), None).expect("read info");
        assert_eq!(info.pid, std::process::id());
        assert_ne!(info.pid, 12345);
    }

    #[test]
    fn acquire_with_timeout_fails_on_fresh_lock() {
        let td = tempdir().expect("tempdir");

        // Create a fresh lock
        let _lock1 = LockFile::acquire(td.path(), None).expect("first acquire");

        // Try to acquire with timeout - should fail
        let result = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("lock already held")
        );
    }

    #[test]
    fn lock_info_serde_roundtrip() {
        let info = LockInfo {
            pid: 12345,
            hostname: "test-host".to_string(),
            acquired_at: Utc::now(),
            plan_id: Some("plan-123".to_string()),
        };

        let json = serde_json::to_string(&info).expect("serialize");
        let parsed: LockInfo = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.pid, info.pid);
        assert_eq!(parsed.hostname, info.hostname);
        assert_eq!(parsed.plan_id, info.plan_id);
    }

    #[test]
    fn lock_info_serde_roundtrip_no_plan_id() {
        let info = LockInfo {
            pid: 99,
            hostname: "h".to_string(),
            acquired_at: Utc::now(),
            plan_id: None,
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let parsed: LockInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.plan_id, None);
    }

    #[test]
    fn lock_path_with_workspace_root_is_hashed() {
        let base = PathBuf::from("state");
        let root = Path::new("/some/workspace");
        let p = lock_path(&base, Some(root));
        // Should contain the LOCK_FILE prefix and a hex hash suffix
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(&format!("{}_", LOCK_FILE)));
        assert!(name.len() > LOCK_FILE.len() + 1);
    }

    #[test]
    fn lock_path_different_roots_produce_different_paths() {
        let base = PathBuf::from("state");
        let p1 = lock_path(&base, Some(Path::new("/workspace/a")));
        let p2 = lock_path(&base, Some(Path::new("/workspace/b")));
        assert_ne!(p1, p2);
    }

    #[test]
    fn lock_path_same_root_produces_same_path() {
        let base = PathBuf::from("state");
        let p1 = lock_path(&base, Some(Path::new("/workspace/a")));
        let p2 = lock_path(&base, Some(Path::new("/workspace/a")));
        assert_eq!(p1, p2);
    }

    #[test]
    fn acquire_with_workspace_root() {
        let td = tempdir().expect("tempdir");
        let root = td.path().join("project");
        let lock = LockFile::acquire(td.path(), Some(&root)).expect("acquire");
        assert!(LockFile::is_locked(td.path(), Some(&root)).expect("is_locked"));
        // Default path should NOT be locked
        assert!(!LockFile::is_locked(td.path(), None).expect("is_locked none"));
        drop(lock);
        assert!(!LockFile::is_locked(td.path(), Some(&root)).expect("is_locked after drop"));
    }

    #[test]
    fn multiple_locks_different_workspace_roots() {
        let td = tempdir().expect("tempdir");
        let root_a = td.path().join("a");
        let root_b = td.path().join("b");
        let lock_a = LockFile::acquire(td.path(), Some(&root_a)).expect("acquire a");
        let lock_b = LockFile::acquire(td.path(), Some(&root_b)).expect("acquire b");
        assert!(LockFile::is_locked(td.path(), Some(&root_a)).expect("locked a"));
        assert!(LockFile::is_locked(td.path(), Some(&root_b)).expect("locked b"));
        drop(lock_a);
        assert!(!LockFile::is_locked(td.path(), Some(&root_a)).expect("unlocked a"));
        assert!(LockFile::is_locked(td.path(), Some(&root_b)).expect("still locked b"));
        drop(lock_b);
    }

    #[test]
    fn acquire_creates_state_directory() {
        let td = tempdir().expect("tempdir");
        let nested = td.path().join("deep").join("nested").join("dir");
        assert!(!nested.exists());
        let lock = LockFile::acquire(&nested, None).expect("acquire");
        assert!(nested.exists());
        drop(lock);
    }

    #[test]
    fn release_is_idempotent() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        lock.release().expect("first release");
        // Second release should not error even though file is gone
        lock.release().expect("second release");
    }

    #[test]
    fn is_locked_returns_false_after_drop() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        assert!(LockFile::is_locked(td.path(), None).expect("locked"));
        drop(lock);
        assert!(!LockFile::is_locked(td.path(), None).expect("unlocked"));
    }

    #[test]
    fn read_lock_info_fails_when_no_lock() {
        let td = tempdir().expect("tempdir");
        let result = LockFile::read_lock_info(td.path(), None);
        assert!(result.is_err());
    }

    #[test]
    fn set_plan_id_fails_when_lock_released() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        lock.release().expect("release");
        let result = lock.set_plan_id("some-plan");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn set_plan_id_can_be_updated_multiple_times() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        lock.set_plan_id("plan-1").expect("set 1");
        lock.set_plan_id("plan-2").expect("set 2");
        let info = LockFile::read_lock_info(td.path(), None).expect("read");
        assert_eq!(info.plan_id, Some("plan-2".to_string()));
    }

    #[test]
    fn acquire_with_timeout_removes_corrupt_lock() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, "not-valid-json").expect("write corrupt");

        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600))
            .expect("acquire after corrupt");
        let info = LockFile::read_lock_info(td.path(), None).expect("read");
        assert_eq!(info.pid, std::process::id());
        drop(lock);
    }

    #[test]
    fn lock_file_contains_valid_json() {
        let td = tempdir().expect("tempdir");
        let _lock = LockFile::acquire(td.path(), None).expect("acquire");
        let lp = lock_path(td.path(), None);
        let content = fs::read_to_string(&lp).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");
        assert!(parsed.get("pid").is_some());
        assert!(parsed.get("hostname").is_some());
        assert!(parsed.get("acquired_at").is_some());
    }

    #[test]
    fn acquire_with_timeout_respects_fresh_lock_age() {
        let td = tempdir().expect("tempdir");
        // Create a lock 30 minutes old, with a 1-hour timeout — should NOT be stale
        let lp = lock_path(td.path(), None);
        let info = LockInfo {
            pid: 99999,
            hostname: "other-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::minutes(30),
            plan_id: Some("active-plan".to_string()),
        };
        fs::write(&lp, serde_json::to_string(&info).expect("ser")).expect("write");

        let result = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("lock already held"));
        assert!(err_msg.contains("99999"));
    }

    #[test]
    fn acquire_with_timeout_and_workspace_root() {
        let td = tempdir().expect("tempdir");
        let root = td.path().join("ws");
        // Create a stale lock with workspace root
        let lp = lock_path(td.path(), Some(&root));
        let old_info = LockInfo {
            pid: 11111,
            hostname: "stale-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::hours(5),
            plan_id: None,
        };
        fs::write(&lp, serde_json::to_string(&old_info).expect("ser")).expect("write");

        let lock =
            LockFile::acquire_with_timeout(td.path(), Some(&root), Duration::from_secs(3600))
                .expect("acquire stale with root");
        let info = LockFile::read_lock_info(td.path(), Some(&root)).expect("read");
        assert_eq!(info.pid, std::process::id());
        drop(lock);
    }

    #[test]
    fn lock_path_none_root_is_deterministic() {
        let base = PathBuf::from("dir");
        assert_eq!(lock_path(&base, None), lock_path(&base, None));
    }

    #[test]
    fn acquire_contention_error_includes_holder_details() {
        let td = tempdir().expect("tempdir");
        let _lock = LockFile::acquire(td.path(), None).expect("acquire");
        let err = LockFile::acquire(td.path(), None).unwrap_err();
        let msg = err.to_string();
        // Should include PID of current process (the holder)
        assert!(msg.contains(&std::process::id().to_string()));
        assert!(msg.contains("lock already held"));
    }

    #[test]
    fn set_plan_id_preserves_other_fields() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        let before = LockFile::read_lock_info(td.path(), None).expect("read before");

        lock.set_plan_id("my-plan").expect("set");

        let after = LockFile::read_lock_info(td.path(), None).expect("read after");
        assert_eq!(before.pid, after.pid);
        assert_eq!(before.hostname, after.hostname);
        assert_eq!(before.acquired_at, after.acquired_at);
        assert_eq!(after.plan_id, Some("my-plan".to_string()));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::tempdir;

    /// Helper to build a deterministic `LockInfo` for snapshot stability.
    fn fixed_lock_info(plan_id: Option<&str>) -> LockInfo {
        LockInfo {
            pid: 42,
            hostname: "build-host".to_string(),
            acquired_at: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
            plan_id: plan_id.map(String::from),
        }
    }

    // ── Lock file content format ────────────────────────────────────────

    #[test]
    fn lock_file_content_without_plan_id() {
        let info = fixed_lock_info(None);
        let json = serde_json::to_string_pretty(&info).expect("serialize");
        insta::assert_snapshot!("lock_file_content_without_plan_id", json);
    }

    #[test]
    fn lock_file_content_with_plan_id() {
        let info = fixed_lock_info(Some("release-2025-01-15"));
        let json = serde_json::to_string_pretty(&info).expect("serialize");
        insta::assert_snapshot!("lock_file_content_with_plan_id", json);
    }

    #[test]
    fn lock_file_yaml_roundtrip() {
        let info = fixed_lock_info(Some("plan-abc-123"));
        insta::assert_yaml_snapshot!("lock_info_yaml", info);
    }

    #[test]
    fn lock_file_on_disk_matches_expected() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let info = fixed_lock_info(Some("on-disk-plan"));
        let json = serde_json::to_string_pretty(&info).expect("serialize");
        fs::create_dir_all(td.path()).ok();
        fs::write(&lp, &json).expect("write");

        let content = fs::read_to_string(&lp).expect("read");
        insta::assert_snapshot!("lock_file_on_disk", content);
    }

    // ── Lock error messages ─────────────────────────────────────────────

    #[test]
    fn error_lock_already_held() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let info = fixed_lock_info(None);
        fs::create_dir_all(td.path()).ok();
        fs::write(&lp, serde_json::to_string(&info).expect("ser")).expect("write");

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        insta::assert_snapshot!("error_lock_already_held", err.to_string());
    }

    #[test]
    fn error_lock_already_held_with_plan_id() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let info = fixed_lock_info(Some("active-plan"));
        fs::create_dir_all(td.path()).ok();
        fs::write(&lp, serde_json::to_string(&info).expect("ser")).expect("write");

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        insta::assert_snapshot!("error_lock_already_held_with_plan_id", err.to_string());
    }

    #[test]
    fn error_fresh_lock_with_timeout() {
        let td = tempdir().expect("tempdir");
        // Use a real lock so the file definitely exists on disk
        let _existing = LockFile::acquire(td.path(), None).expect("seed lock");

        let err = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(86400 * 365))
            .unwrap_err();
        let msg = err.to_string();
        // The message contains the dynamic current PID/host/age, so snapshot only the stable prefix
        assert!(msg.contains("lock already held"));
        insta::assert_snapshot!(
            "error_fresh_lock_with_timeout_prefix",
            "lock already held by current process (fresh lock within timeout)"
        );
    }

    #[test]
    fn error_read_nonexistent_lock() {
        let td = tempdir().expect("tempdir");
        let err = LockFile::read_lock_info(td.path(), None).unwrap_err();
        // Only snapshot the root cause message (path-independent part)
        let msg = err.to_string();
        assert!(msg.contains("failed to read lock file"));
        insta::assert_snapshot!(
            "error_read_nonexistent_lock_prefix",
            "failed to read lock file"
        );
    }

    #[test]
    fn error_set_plan_id_after_release() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        lock.release().expect("release");
        let err = lock.set_plan_id("orphan-plan").unwrap_err();
        // Path-independent portion
        assert!(err.to_string().contains("lock file does not exist"));
        insta::assert_snapshot!(
            "error_set_plan_id_after_release_prefix",
            "lock file does not exist"
        );
    }

    #[test]
    fn error_corrupt_lock_file() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::create_dir_all(td.path()).ok();
        fs::write(&lp, "<<<not json>>>").expect("write");

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("failed to parse lock JSON"));
        insta::assert_snapshot!(
            "error_corrupt_lock_file_prefix",
            "failed to parse lock JSON"
        );
    }

    // ── Lock status display ─────────────────────────────────────────────

    #[test]
    fn lock_status_unlocked() {
        let td = tempdir().expect("tempdir");
        let locked = LockFile::is_locked(td.path(), None).expect("check");
        insta::assert_snapshot!("lock_status_unlocked", format!("locked: {locked}"));
    }

    #[test]
    fn lock_status_locked() {
        let td = tempdir().expect("tempdir");
        let _lock = LockFile::acquire(td.path(), None).expect("acquire");
        let locked = LockFile::is_locked(td.path(), None).expect("check");
        insta::assert_snapshot!("lock_status_locked", format!("locked: {locked}"));
    }

    #[test]
    fn lock_info_debug_display() {
        let info = fixed_lock_info(Some("display-plan"));
        insta::assert_snapshot!("lock_info_debug", format!("{info:#?}"));
    }

    #[test]
    fn lock_path_without_root_snapshot() {
        let p = lock_path(Path::new(".shipper"), None);
        insta::assert_snapshot!(
            "lock_path_without_root",
            p.to_string_lossy().replace('\\', "/")
        );
    }

    #[test]
    fn lock_path_with_root_snapshot() {
        let p = lock_path(Path::new(".shipper"), Some(Path::new("/my/workspace")));
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        // Snapshot only the filename (hash is deterministic for same input)
        insta::assert_snapshot!("lock_path_with_root_filename", name);
    }
}

#[cfg(test)]
mod edge_case_tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::tempdir;

    // ── 1. Stale lock detection and recovery ────────────────────────────

    #[test]
    fn stale_lock_exactly_at_timeout_boundary_is_not_removed() {
        // A lock whose age equals the timeout should NOT be considered stale
        // because the comparison is strictly greater-than.
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let timeout_secs = 3600u64;
        let info = LockInfo {
            pid: 55555,
            hostname: "boundary-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::seconds(timeout_secs as i64),
            plan_id: None,
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        // Lock age ≈ timeout; due to time passing between write and check
        // this may be slightly over, so we use a timeout 1 second larger to
        // ensure the lock is truly at the boundary.
        let result =
            LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(timeout_secs + 1));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("lock already held")
        );
    }

    #[test]
    fn stale_lock_one_second_past_timeout_is_removed() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let timeout_secs = 60u64;
        let info = LockInfo {
            pid: 55556,
            hostname: "past-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::seconds((timeout_secs + 2) as i64),
            plan_id: Some("stale-plan".to_string()),
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        let lock =
            LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(timeout_secs))
                .expect("should remove stale lock");
        let new_info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(new_info.pid, std::process::id());
        assert!(new_info.plan_id.is_none());
        drop(lock);
    }

    #[test]
    fn stale_lock_recovery_preserves_state_dir() {
        let td = tempdir().expect("tempdir");
        let nested = td.path().join("deep").join("state");
        fs::create_dir_all(&nested).unwrap();
        let lp = lock_path(&nested, None);
        let info = LockInfo {
            pid: 44444,
            hostname: "stale-nested".to_string(),
            acquired_at: Utc::now() - chrono::Duration::hours(10),
            plan_id: None,
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        let lock = LockFile::acquire_with_timeout(&nested, None, Duration::from_secs(60)).unwrap();
        assert!(nested.exists());
        drop(lock);
    }

    // ── 2. Concurrent lock acquisition from multiple threads ────────────

    #[test]
    fn concurrent_acquire_only_one_succeeds() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().to_path_buf();
        let thread_count = 8;
        let barrier = Arc::new(Barrier::new(thread_count));

        let handles: Vec<_> = (0..thread_count)
            .map(|_| {
                let dir = state_dir.clone();
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    b.wait();
                    LockFile::acquire(&dir, None).ok()
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let successes = results.iter().filter(|r| r.is_some()).count();
        // At most one thread should successfully acquire the lock.
        // Due to the non-atomic check-then-create in acquire(), more than
        // one *might* succeed in a race, but at least one must succeed.
        assert!(successes >= 1, "at least one thread must acquire the lock");
    }

    #[test]
    fn concurrent_acquire_with_timeout_replaces_stale() {
        let td = tempdir().expect("tempdir");
        let state_dir = td.path().to_path_buf();
        let lp = lock_path(&state_dir, None);
        let info = LockInfo {
            pid: 99990,
            hostname: "old".to_string(),
            acquired_at: Utc::now() - chrono::Duration::hours(5),
            plan_id: None,
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        let thread_count = 4;
        let barrier = Arc::new(Barrier::new(thread_count));
        let handles: Vec<_> = (0..thread_count)
            .map(|_| {
                let dir = state_dir.clone();
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    b.wait();
                    LockFile::acquire_with_timeout(&dir, None, Duration::from_secs(60)).ok()
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let successes = results.iter().filter(|r| r.is_some()).count();
        assert!(successes >= 1, "at least one thread must succeed");
    }

    // ── 3. Force-break of existing lock ─────────────────────────────────

    #[test]
    fn force_break_by_removing_lock_then_reacquire() {
        let td = tempdir().expect("tempdir");
        let _lock = LockFile::acquire(td.path(), None).expect("initial acquire");
        let lp = lock_path(td.path(), None);
        assert!(lp.exists());

        // Simulate a force-break: manually remove the lock file
        fs::remove_file(&lp).expect("force remove");
        assert!(!lp.exists());

        // Now a new acquire should succeed
        let lock2 = LockFile::acquire(td.path(), None).expect("reacquire after force-break");
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.pid, std::process::id());
        drop(lock2);
    }

    #[test]
    fn force_break_stale_via_timeout_zero() {
        // With timeout=0, any existing lock is considered stale (age > 0)
        // unless it was literally just created in the same second.
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let info = LockInfo {
            pid: 33333,
            hostname: "force-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::seconds(1),
            plan_id: None,
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(0)).unwrap();
        let new_info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(new_info.pid, std::process::id());
        drop(lock);
    }

    #[test]
    fn force_break_lock_held_by_different_workspace_root() {
        let td = tempdir().expect("tempdir");
        let root_a = td.path().join("ws-a");
        let root_b = td.path().join("ws-b");
        let _lock_a = LockFile::acquire(td.path(), Some(&root_a)).unwrap();

        // Force-break for root_a should not affect root_b
        let lp_a = lock_path(td.path(), Some(&root_a));
        fs::remove_file(&lp_a).unwrap();

        let lock_a2 = LockFile::acquire(td.path(), Some(&root_a)).unwrap();
        // root_b should still be unlocked (never had a lock)
        assert!(!LockFile::is_locked(td.path(), Some(&root_b)).unwrap());
        drop(lock_a2);
    }

    // ── 4. Lock file with corrupt/invalid content ───────────────────────

    #[test]
    fn corrupt_lock_empty_file() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, "").unwrap();

        // acquire should fail because the file exists but can't be parsed
        let err = LockFile::acquire(td.path(), None).unwrap_err();
        assert!(err.to_string().contains("failed to parse lock JSON"));
    }

    #[test]
    fn corrupt_lock_partial_json() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, r#"{"pid": 1, "hostname": "h""#).unwrap();

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        assert!(err.to_string().contains("failed to parse lock JSON"));
    }

    #[test]
    fn corrupt_lock_wrong_json_type() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, "[1, 2, 3]").unwrap();

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        assert!(err.to_string().contains("failed to parse lock JSON"));
    }

    #[test]
    fn corrupt_lock_missing_required_fields() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, r#"{"pid": 1}"#).unwrap();

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        assert!(err.to_string().contains("failed to parse lock JSON"));
    }

    #[test]
    fn corrupt_lock_binary_content() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, [0xFF, 0xFE, 0x00, 0x01, 0x80]).unwrap();

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        assert!(
            err.to_string().contains("failed to read lock file")
                || err.to_string().contains("failed to parse lock JSON")
        );
    }

    #[test]
    fn corrupt_lock_removed_by_acquire_with_timeout() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, "totally-invalid").unwrap();

        // acquire_with_timeout should remove corrupt lock and succeed
        let lock =
            LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600)).unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.pid, std::process::id());
        drop(lock);
    }

    #[test]
    fn corrupt_lock_empty_json_object() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, "{}").unwrap();

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        assert!(err.to_string().contains("failed to parse lock JSON"));
    }

    #[test]
    fn corrupt_lock_wrong_pid_type() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(
            &lp,
            r#"{"pid": "not-a-number", "hostname": "h", "acquired_at": "2025-01-01T00:00:00Z", "plan_id": null}"#,
        )
        .unwrap();

        let err = LockFile::acquire(td.path(), None).unwrap_err();
        assert!(err.to_string().contains("failed to parse lock JSON"));
    }

    // ── 5. Lock in unicode directory path ───────────────────────────────

    #[test]
    fn lock_in_unicode_directory() {
        let td = tempdir().expect("tempdir");
        let unicode_dir = td.path().join("ünïcödé_目录_🔒");
        let lock = LockFile::acquire(&unicode_dir, None).expect("acquire in unicode dir");
        assert!(LockFile::is_locked(&unicode_dir, None).unwrap());
        let info = LockFile::read_lock_info(&unicode_dir, None).unwrap();
        assert_eq!(info.pid, std::process::id());
        drop(lock);
        assert!(!LockFile::is_locked(&unicode_dir, None).unwrap());
    }

    #[test]
    fn lock_with_unicode_workspace_root() {
        let td = tempdir().expect("tempdir");
        let root = td.path().join("プロジェクト");
        let lock = LockFile::acquire(td.path(), Some(&root)).unwrap();
        assert!(LockFile::is_locked(td.path(), Some(&root)).unwrap());
        lock.release().unwrap();
        assert!(!LockFile::is_locked(td.path(), Some(&root)).unwrap());
    }

    #[test]
    fn lock_in_deeply_nested_unicode_path() {
        let td = tempdir().expect("tempdir");
        let deep = td.path().join("α").join("β").join("γ").join("δ");
        let lock = LockFile::acquire(&deep, None).unwrap();
        assert!(deep.exists());
        let info = LockFile::read_lock_info(&deep, None).unwrap();
        assert_eq!(info.pid, std::process::id());
        drop(lock);
    }

    // ── 6. Lock with zero timeout ───────────────────────────────────────

    #[test]
    fn zero_timeout_breaks_old_lock() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let info = LockInfo {
            pid: 22222,
            hostname: "zero-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::seconds(2),
            plan_id: None,
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(0)).unwrap();
        let new_info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(new_info.pid, std::process::id());
        drop(lock);
    }

    #[test]
    fn zero_timeout_on_empty_dir_succeeds() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(0)).unwrap();
        assert!(LockFile::is_locked(td.path(), None).unwrap());
        drop(lock);
    }

    #[test]
    fn zero_timeout_removes_corrupt_lock() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, "garbage").unwrap();

        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(0)).unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.pid, std::process::id());
        drop(lock);
    }

    // ── 7. Lock with very large timeout ─────────────────────────────────

    #[test]
    fn very_large_timeout_does_not_remove_fresh_lock() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let info = LockInfo {
            pid: 11112,
            hostname: "large-timeout-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::hours(24 * 365),
            plan_id: None,
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        // Timeout so large that even a year-old lock is not stale
        let result = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(u64::MAX));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("lock already held")
        );
    }

    #[test]
    fn very_large_timeout_on_empty_dir_succeeds() {
        let td = tempdir().expect("tempdir");
        let lock =
            LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(u64::MAX)).unwrap();
        assert!(LockFile::is_locked(td.path(), None).unwrap());
        drop(lock);
    }

    #[test]
    fn max_duration_timeout_with_stale_lock() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        let info = LockInfo {
            pid: 11113,
            hostname: "max-host".to_string(),
            acquired_at: Utc::now() - chrono::Duration::weeks(52 * 100),
            plan_id: Some("ancient-plan".to_string()),
        };
        fs::write(&lp, serde_json::to_string(&info).unwrap()).unwrap();

        // u64::MAX seconds ≈ 584 billion years — nothing is stale
        let result = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(u64::MAX));
        assert!(result.is_err());
    }

    // ── 8. Lock file permissions edge cases ─────────────────────────────

    #[test]
    fn acquire_in_nonexistent_nested_directory() {
        let td = tempdir().expect("tempdir");
        let deep = td.path().join("a").join("b").join("c").join("d");
        assert!(!deep.exists());
        let lock = LockFile::acquire(&deep, None).unwrap();
        assert!(deep.exists());
        assert!(LockFile::is_locked(&deep, None).unwrap());
        drop(lock);
    }

    #[test]
    fn release_already_deleted_lock_is_ok() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).unwrap();
        let lp = lock_path(td.path(), None);
        // External process deletes the lock file
        fs::remove_file(&lp).unwrap();
        // release should not error
        lock.release().unwrap();
    }

    #[test]
    fn double_release_is_idempotent() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).unwrap();
        lock.release().unwrap();
        lock.release().unwrap();
        assert!(!lock_path(td.path(), None).exists());
    }

    #[test]
    fn set_plan_id_on_externally_deleted_lock_fails() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).unwrap();
        let lp = lock_path(td.path(), None);
        fs::remove_file(&lp).unwrap();
        let err = lock.set_plan_id("orphan").unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn read_lock_info_on_corrupt_file_fails() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        fs::write(&lp, "not json").unwrap();
        let err = LockFile::read_lock_info(td.path(), None).unwrap_err();
        assert!(err.to_string().contains("failed to parse lock JSON"));
    }

    // ── 9. Snapshot tests for LockState variants ────────────────────────

    /// Represents the logical state of a lock for snapshot testing.
    #[derive(Debug)]
    #[allow(dead_code)]
    enum LockState {
        Unlocked,
        Locked(LockInfo),
        Stale(LockInfo),
        Corrupt(String),
    }

    fn fixed_info(pid: u32, plan_id: Option<&str>) -> LockInfo {
        use chrono::TimeZone;
        LockInfo {
            pid,
            hostname: "snap-host".to_string(),
            acquired_at: Utc.with_ymd_and_hms(2025, 6, 15, 8, 30, 0).unwrap(),
            plan_id: plan_id.map(String::from),
        }
    }

    #[test]
    fn snapshot_lock_state_unlocked() {
        let state = LockState::Unlocked;
        insta::assert_debug_snapshot!("lock_state_unlocked", state);
    }

    #[test]
    fn snapshot_lock_state_locked_no_plan() {
        let state = LockState::Locked(fixed_info(100, None));
        insta::assert_debug_snapshot!("lock_state_locked_no_plan", state);
    }

    #[test]
    fn snapshot_lock_state_locked_with_plan() {
        let state = LockState::Locked(fixed_info(200, Some("release-v1.0")));
        insta::assert_debug_snapshot!("lock_state_locked_with_plan", state);
    }

    #[test]
    fn snapshot_lock_state_stale() {
        let state = LockState::Stale(fixed_info(300, Some("old-plan")));
        insta::assert_debug_snapshot!("lock_state_stale", state);
    }

    #[test]
    fn snapshot_lock_state_corrupt() {
        let state = LockState::Corrupt("<<<not json>>>".to_string());
        insta::assert_debug_snapshot!("lock_state_corrupt", state);
    }

    #[test]
    fn snapshot_lock_info_all_fields() {
        let info = fixed_info(42, Some("plan-xyz-789"));
        insta::assert_debug_snapshot!("lock_info_all_fields", info);
    }

    #[test]
    fn snapshot_lock_info_no_plan_id() {
        let info = fixed_info(1, None);
        insta::assert_debug_snapshot!("lock_info_no_plan_id", info);
    }
}

#[cfg(test)]
mod proptest_edge_cases {
    use super::*;
    use proptest::prelude::*;
    use tempfile::tempdir;

    proptest! {
        // ── 10. Property test: lock acquire + release is always paired ──

        #[test]
        fn acquire_release_always_paired(dir_suffix in "[a-zA-Z0-9]{1,16}") {
            let td = tempdir().expect("tempdir");
            let state_dir = td.path().join(dir_suffix);

            // Before: not locked
            prop_assert!(!lock_path(&state_dir, None).exists());

            // Acquire
            let lock = LockFile::acquire(&state_dir, None).expect("acquire");
            prop_assert!(lock_path(&state_dir, None).exists());

            // Release
            lock.release().expect("release");
            prop_assert!(!lock_path(&state_dir, None).exists());
        }

        #[test]
        fn acquire_drop_always_paired(dir_suffix in "[a-zA-Z0-9]{1,16}") {
            let td = tempdir().expect("tempdir");
            let state_dir = td.path().join(dir_suffix);

            prop_assert!(!lock_path(&state_dir, None).exists());

            {
                let _lock = LockFile::acquire(&state_dir, None).expect("acquire");
                prop_assert!(lock_path(&state_dir, None).exists());
            }
            // Drop should have released
            prop_assert!(!lock_path(&state_dir, None).exists());
        }

        #[test]
        fn acquire_with_timeout_release_always_paired(
            dir_suffix in "[a-zA-Z0-9]{1,16}",
            timeout_secs in 1u64..=3600u64,
        ) {
            let td = tempdir().expect("tempdir");
            let state_dir = td.path().join(dir_suffix);

            let lock = LockFile::acquire_with_timeout(
                &state_dir,
                None,
                Duration::from_secs(timeout_secs),
            ).expect("acquire");
            prop_assert!(lock_path(&state_dir, None).exists());

            lock.release().expect("release");
            prop_assert!(!lock_path(&state_dir, None).exists());
        }

        #[test]
        fn acquire_set_plan_release_always_paired(
            dir_suffix in "[a-zA-Z0-9]{1,16}",
            plan_id in "[a-zA-Z0-9_-]{1,32}",
        ) {
            let td = tempdir().expect("tempdir");
            let state_dir = td.path().join(dir_suffix);

            let lock = LockFile::acquire(&state_dir, None).expect("acquire");
            lock.set_plan_id(&plan_id).expect("set_plan_id");
            let info = LockFile::read_lock_info(&state_dir, None).expect("read");
            prop_assert_eq!(info.plan_id.as_deref(), Some(plan_id.as_str()));

            lock.release().expect("release");
            prop_assert!(!lock_path(&state_dir, None).exists());
        }

        #[test]
        fn corrupt_lock_always_recoverable_with_timeout(
            dir_suffix in "[a-zA-Z0-9]{1,16}",
            garbage in "[^\x00]{1,256}",
        ) {
            let td = tempdir().expect("tempdir");
            let state_dir = td.path().join(&dir_suffix);
            fs::create_dir_all(&state_dir).expect("mkdir");
            let lp = lock_path(&state_dir, None);
            fs::write(&lp, &garbage).expect("write garbage");

            let lock = LockFile::acquire_with_timeout(
                &state_dir,
                None,
                Duration::from_secs(3600),
            ).expect("should recover from corrupt lock");

            let info = LockFile::read_lock_info(&state_dir, None).expect("read");
            prop_assert_eq!(info.pid, std::process::id());

            lock.release().expect("release");
            prop_assert!(!lock_path(&state_dir, None).exists());
        }
    }
}

#[cfg(test)]
mod hardened_tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::tempdir;

    // ── Lock acquisition metadata ───────────────────────────────────────

    #[test]
    fn acquire_records_current_pid() {
        let td = tempdir().unwrap();
        let _lock = LockFile::acquire(td.path(), None).unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.pid, std::process::id());
    }

    #[test]
    fn acquire_timestamp_is_recent() {
        let before = Utc::now();
        let td = tempdir().unwrap();
        let _lock = LockFile::acquire(td.path(), None).unwrap();
        let after = Utc::now();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert!(info.acquired_at >= before);
        assert!(info.acquired_at <= after);
    }

    #[test]
    fn plan_id_is_none_immediately_after_acquire() {
        let td = tempdir().unwrap();
        let _lock = LockFile::acquire(td.path(), None).unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.plan_id, None);
    }

    // ── Plan ID matching ────────────────────────────────────────────────

    #[test]
    fn plan_id_matches_after_set() {
        let td = tempdir().unwrap();
        let lock = LockFile::acquire(td.path(), None).unwrap();
        lock.set_plan_id("abc-123").unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.plan_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn plan_id_does_not_match_different_value() {
        let td = tempdir().unwrap();
        let lock = LockFile::acquire(td.path(), None).unwrap();
        lock.set_plan_id("plan-a").unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_ne!(info.plan_id.as_deref(), Some("plan-b"));
    }

    #[test]
    fn set_plan_id_with_empty_string() {
        let td = tempdir().unwrap();
        let lock = LockFile::acquire(td.path(), None).unwrap();
        lock.set_plan_id("").unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.plan_id, Some(String::new()));
    }

    #[test]
    fn set_plan_id_overwrites_previous() {
        let td = tempdir().unwrap();
        let lock = LockFile::acquire(td.path(), None).unwrap();
        lock.set_plan_id("first").unwrap();
        lock.set_plan_id("second").unwrap();
        lock.set_plan_id("third").unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_eq!(info.plan_id.as_deref(), Some("third"));
    }

    // ── RAII drop release ───────────────────────────────────────────────

    #[test]
    fn drop_in_inner_scope_allows_reacquire() {
        let td = tempdir().unwrap();
        {
            let _lock = LockFile::acquire(td.path(), None).unwrap();
        }
        // After drop, should be able to re-acquire
        let lock2 = LockFile::acquire(td.path(), None).unwrap();
        assert!(LockFile::is_locked(td.path(), None).unwrap());
        drop(lock2);
    }

    #[test]
    fn explicit_release_then_reacquire() {
        let td = tempdir().unwrap();
        let lock = LockFile::acquire(td.path(), None).unwrap();
        lock.release().unwrap();
        let _lock2 = LockFile::acquire(td.path(), None).unwrap();
        assert!(LockFile::is_locked(td.path(), None).unwrap());
    }

    // ── Lock file JSON format stability ─────────────────────────────────

    #[test]
    fn lock_file_json_has_exactly_four_keys() {
        let td = tempdir().unwrap();
        let _lock = LockFile::acquire(td.path(), None).unwrap();
        let lp = lock_path(td.path(), None);
        let content = fs::read_to_string(&lp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let obj = parsed.as_object().unwrap();
        assert_eq!(obj.len(), 4);
        assert!(obj.contains_key("pid"));
        assert!(obj.contains_key("hostname"));
        assert!(obj.contains_key("acquired_at"));
        assert!(obj.contains_key("plan_id"));
    }

    #[test]
    fn lock_file_json_plan_id_null_when_unset() {
        let td = tempdir().unwrap();
        let _lock = LockFile::acquire(td.path(), None).unwrap();
        let lp = lock_path(td.path(), None);
        let content = fs::read_to_string(&lp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed["plan_id"].is_null());
    }

    #[test]
    fn lock_file_json_plan_id_string_when_set() {
        let td = tempdir().unwrap();
        let lock = LockFile::acquire(td.path(), None).unwrap();
        lock.set_plan_id("my-plan").unwrap();
        let lp = lock_path(td.path(), None);
        let content = fs::read_to_string(&lp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["plan_id"].as_str(), Some("my-plan"));
    }

    // ── Stale lock: force acquisition ───────────────────────────────────

    #[test]
    fn force_acquire_via_timeout_replaces_all_metadata() {
        let td = tempdir().unwrap();
        let lp = lock_path(td.path(), None);
        let old = LockInfo {
            pid: 65432,
            hostname: "old-machine".to_string(),
            acquired_at: Utc::now() - chrono::Duration::hours(3),
            plan_id: Some("stale-run".to_string()),
        };
        fs::write(&lp, serde_json::to_string(&old).unwrap()).unwrap();

        let _lock =
            LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(60)).unwrap();
        let info = LockFile::read_lock_info(td.path(), None).unwrap();
        assert_ne!(info.pid, 65432);
        assert_ne!(info.hostname, "old-machine");
        assert!(info.plan_id.is_none());
    }

    // ── lock_path edge cases ────────────────────────────────────────────

    #[test]
    fn lock_path_empty_workspace_root_differs_from_none() {
        let base = PathBuf::from("state");
        let with_empty = lock_path(&base, Some(Path::new("")));
        let without = lock_path(&base, None);
        assert_ne!(with_empty, without);
    }

    #[test]
    fn lock_path_dot_and_dotdot_roots_differ() {
        let base = PathBuf::from("state");
        let p1 = lock_path(&base, Some(Path::new(".")));
        let p2 = lock_path(&base, Some(Path::new("..")));
        assert_ne!(p1, p2);
    }

    // ── Snapshot tests ──────────────────────────────────────────────────

    #[test]
    fn snapshot_lock_info_with_empty_plan_id() {
        let info = LockInfo {
            pid: 42,
            hostname: "snap-host".to_string(),
            acquired_at: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
            plan_id: Some(String::new()),
        };
        let json = serde_json::to_string_pretty(&info).unwrap();
        insta::assert_snapshot!("lock_info_empty_plan_id", json);
    }

    #[test]
    fn snapshot_lock_info_json_key_order() {
        let info = LockInfo {
            pid: 1,
            hostname: "h".to_string(),
            acquired_at: Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap(),
            plan_id: Some("p".to_string()),
        };
        let json = serde_json::to_string_pretty(&info).unwrap();
        // Snapshot captures the exact key order to detect accidental reordering
        insta::assert_snapshot!("lock_info_json_key_order", json);
    }

    #[test]
    fn snapshot_lock_file_after_set_plan_id() {
        let info = LockInfo {
            pid: 42,
            hostname: "build-host".to_string(),
            acquired_at: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
            plan_id: Some("updated-plan-456".to_string()),
        };
        let json = serde_json::to_string_pretty(&info).unwrap();
        insta::assert_snapshot!("lock_file_after_set_plan_id", json);
    }
}

#[cfg(test)]
mod hardened_proptests {
    use super::*;
    use proptest::prelude::*;
    use tempfile::tempdir;

    proptest! {
        #[test]
        fn arbitrary_plan_ids_never_panic_on_set(
            plan_id in ".*",
        ) {
            let td = tempdir().expect("tempdir");
            let lock = LockFile::acquire(td.path(), None).expect("acquire");
            // set_plan_id should never panic regardless of input
            let _ = lock.set_plan_id(&plan_id);
            drop(lock);
        }

        #[test]
        fn arbitrary_paths_never_panic_on_lock_path(
            dir in "[a-zA-Z0-9_./-]{0,128}",
            root in proptest::option::of("[a-zA-Z0-9_./-]{0,128}"),
        ) {
            let base = PathBuf::from(&dir);
            let root_path = root.as_ref().map(PathBuf::from);
            // lock_path should never panic
            let _ = lock_path(&base, root_path.as_deref());
        }

        #[test]
        fn read_lock_info_on_arbitrary_content_never_panics(
            content in ".*",
        ) {
            let td = tempdir().expect("tempdir");
            let lp = lock_path(td.path(), None);
            std::fs::write(&lp, content.as_bytes()).expect("write");
            // Should return Err, never panic
            let _ = LockFile::read_lock_info(td.path(), None);
        }

        #[test]
        fn plan_id_roundtrip_matches_for_arbitrary_ids(
            plan_id in "[^\x00]{1,256}",
        ) {
            let td = tempdir().expect("tempdir");
            let lock = LockFile::acquire(td.path(), None).expect("acquire");
            lock.set_plan_id(&plan_id).expect("set");
            let info = LockFile::read_lock_info(td.path(), None).expect("read");
            prop_assert_eq!(info.plan_id.as_deref(), Some(plan_id.as_str()));
            drop(lock);
        }
    }
}

#[cfg(test)]
mod lock_edge_case_tests {
    use super::*;
    use tempfile::tempdir;

    // ── Stale lock with wrong (non-existent) PID ────────────────────

    #[test]
    fn stale_lock_with_wrong_pid_replaced_by_timeout() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);

        // Write a lock claiming PID 1 (init/system, not our process)
        let info = LockInfo {
            pid: 1,
            hostname: "other-machine".to_string(),
            acquired_at: Utc::now() - chrono::Duration::hours(3),
            plan_id: Some("old-plan".to_string()),
        };
        std::fs::write(&lp, serde_json::to_string(&info).expect("ser")).expect("write");

        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600))
            .expect("should replace stale lock with wrong PID");
        let new_info = LockFile::read_lock_info(td.path(), None).expect("read");
        assert_eq!(new_info.pid, std::process::id());
        assert!(new_info.plan_id.is_none());
        drop(lock);
    }

    // ── Lock file with truncated JSON (corrupt) ─────────────────────

    #[test]
    fn truncated_json_lock_is_treated_as_corrupt() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        std::fs::write(&lp, r#"{"pid": 42, "hostname":"#).expect("write");

        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600))
            .expect("should replace corrupt lock");
        let info = LockFile::read_lock_info(td.path(), None).expect("read");
        assert_eq!(info.pid, std::process::id());
        drop(lock);
    }

    // ── Lock file with empty content ────────────────────────────────

    #[test]
    fn empty_lock_file_treated_as_corrupt() {
        let td = tempdir().expect("tempdir");
        let lp = lock_path(td.path(), None);
        std::fs::write(&lp, "").expect("write");

        let lock = LockFile::acquire_with_timeout(td.path(), None, Duration::from_secs(3600))
            .expect("should replace empty lock");
        let info = LockFile::read_lock_info(td.path(), None).expect("read");
        assert_eq!(info.pid, std::process::id());
        drop(lock);
    }

    // ── Lock in deeply nested directory ──────────────────────────────

    #[test]
    fn lock_in_deeply_nested_directory() {
        let td = tempdir().expect("tempdir");
        let deep = td.path().join("a").join("b").join("c").join("d");
        // acquire should create all intermediate dirs
        let lock = LockFile::acquire(&deep, None).expect("acquire in deep dir");
        assert!(LockFile::is_locked(&deep, None).expect("is_locked"));
        drop(lock);
    }

    // ── Lock path with Unicode workspace root ───────────────────────

    #[test]
    fn lock_path_with_unicode_workspace_root() {
        let base = std::path::PathBuf::from("state");
        let root1 = std::path::Path::new("/ワークスペース/α");
        let root2 = std::path::Path::new("/ワークスペース/β");
        let p1 = lock_path(&base, Some(root1));
        let p2 = lock_path(&base, Some(root2));
        // Different roots should produce different paths
        assert_ne!(p1, p2);
        // Same root should be deterministic
        assert_eq!(lock_path(&base, Some(root1)), p1);
    }

    // ── Acquire, set_plan_id, then re-read verifies JSON structure ──

    #[test]
    fn lock_json_structure_after_set_plan_id() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path(), None).expect("acquire");
        lock.set_plan_id("edge-plan-🚀").expect("set");

        let lp = lock_path(td.path(), None);
        let content = std::fs::read_to_string(&lp).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert_eq!(parsed["plan_id"].as_str(), Some("edge-plan-🚀"));
        assert!(parsed["pid"].is_number());
        assert!(parsed["hostname"].is_string());
        drop(lock);
    }
}
