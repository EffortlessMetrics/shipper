//! File-based locking mechanism to prevent concurrent operations.
//!
//! This crate provides a simple file-based lock that can be used to prevent
//! concurrent access to shared resources across processes. The lock file
//! contains metadata about the lock holder (PID, hostname, timestamp).
//!
//! # Example
//!
//! ```
//! use shipper_lock::LockFile;
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
    /// use shipper_lock::LockFile;
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
    /// use shipper_lock::LockFile;
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
