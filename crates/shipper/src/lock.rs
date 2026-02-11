//! Lock file mechanism to prevent concurrent publishes.
//!
//! The lock file is stored in `.shipper/lock` and contains JSON metadata
//! about the lock holder (PID, hostname, timestamp, plan_id).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    file: Option<File>,
}

impl LockFile {
    /// Acquire a lock file in the specified state directory
    ///
    /// This will fail if a lock already exists and is not stale.
    /// Use `is_locked` first to check, or use `acquire_with_timeout` for
    /// automatic stale lock handling.
    pub fn acquire(state_dir: &Path) -> Result<Self> {
        let lock_path = state_dir.join(LOCK_FILE);

        // Create state directory if it doesn't exist
        fs::create_dir_all(state_dir)
            .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;

        // Check if lock already exists
        if lock_path.exists() {
            let existing_info = Self::read_lock_info(state_dir)?;
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

        Ok(Self {
            path: lock_path,
            file: None,
        })
    }

    /// Acquire a lock, automatically removing stale locks older than timeout
    pub fn acquire_with_timeout(state_dir: &Path, timeout: Duration) -> Result<Self> {
        let lock_path = state_dir.join(LOCK_FILE);

        if lock_path.exists() {
            if let Ok(info) = Self::read_lock_info(state_dir) {
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

        Self::acquire(state_dir)
    }

    /// Release the lock file
    pub fn release(&mut self) -> Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)
                .with_context(|| format!("failed to remove lock file {}", self.path.display()))?;
        }
        self.file = None;
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
    pub fn is_locked(state_dir: &Path) -> Result<bool> {
        Ok(state_dir.join(LOCK_FILE).exists())
    }

    /// Read the lock file information
    pub fn read_lock_info(state_dir: &Path) -> Result<LockInfo> {
        read_lock_info_from_path(&state_dir.join(LOCK_FILE))
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

/// Get the lock file path for a state directory
pub fn lock_path(state_dir: &Path) -> PathBuf {
    state_dir.join(LOCK_FILE)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn lock_path_returns_expected_path() {
        let base = PathBuf::from("x");
        assert_eq!(lock_path(&base), PathBuf::from("x").join(LOCK_FILE));
    }

    #[test]
    fn acquire_creates_lock_file() {
        let td = tempdir().expect("tempdir");
        let mut lock = LockFile::acquire(td.path()).expect("acquire");
        assert!(lock_path(td.path()).exists());
        lock.release().expect("release");
        assert!(!lock_path(td.path()).exists());
    }

    #[test]
    fn acquire_fails_when_locked() {
        let td = tempdir().expect("tempdir");
        let _lock1 = LockFile::acquire(td.path()).expect("first acquire");

        let result = LockFile::acquire(td.path());
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
            let _lock = LockFile::acquire(td.path()).expect("acquire");
            assert!(lock_path(td.path()).exists());
        }
        // Lock should be released after drop
        assert!(!lock_path(td.path()).exists());
    }

    #[test]
    fn read_lock_info_returns_correct_info() {
        let td = tempdir().expect("tempdir");
        let _lock = LockFile::acquire(td.path()).expect("acquire");

        let info = LockFile::read_lock_info(td.path()).expect("read info");
        assert_eq!(info.pid, std::process::id());
        assert!(!info.hostname.is_empty());
        assert!(info.plan_id.is_none());
    }

    #[test]
    fn set_plan_id_updates_lock() {
        let td = tempdir().expect("tempdir");
        let lock = LockFile::acquire(td.path()).expect("acquire");

        lock.set_plan_id("test-plan-123").expect("set plan_id");

        let info = LockFile::read_lock_info(td.path()).expect("read info");
        assert_eq!(info.plan_id, Some("test-plan-123".to_string()));
    }

    #[test]
    fn is_locked_returns_correct_status() {
        let td = tempdir().expect("tempdir");
        assert!(!LockFile::is_locked(td.path()).expect("is_locked"));

        let _lock = LockFile::acquire(td.path()).expect("acquire");
        assert!(LockFile::is_locked(td.path()).expect("is_locked"));
    }

    #[test]
    fn acquire_with_timeout_removes_stale_locks() {
        let td = tempdir().expect("tempdir");

        // Create a lock with old timestamp
        let lock_path = lock_path(td.path());
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
        let _lock = LockFile::acquire_with_timeout(td.path(), Duration::from_secs(3600))
            .expect("acquire with timeout");

        let info = LockFile::read_lock_info(td.path()).expect("read info");
        assert_eq!(info.pid, std::process::id());
        assert_ne!(info.pid, 12345);
    }

    #[test]
    fn acquire_with_timeout_fails_on_fresh_lock() {
        let td = tempdir().expect("tempdir");

        // Create a fresh lock
        let _lock1 = LockFile::acquire(td.path()).expect("first acquire");

        // Try to acquire with timeout - should fail
        let result = LockFile::acquire_with_timeout(td.path(), Duration::from_secs(3600));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("lock already held")
        );
    }
}
