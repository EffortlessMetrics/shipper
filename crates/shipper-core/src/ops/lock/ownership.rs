use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    LockContents, LockInfo, LockRecord, current_process_identity, lock_path, process_identity,
    process_scope, read_lock_info_from_path,
};

const OWNERSHIP_PROTOCOL: &str = "shipper-os-lock-v1";

#[derive(Debug, Serialize, Deserialize)]
struct OwnershipMetadata {
    protocol: String,
    owner: String,
}

impl OwnershipMetadata {
    fn recognized(&self) -> bool {
        self.protocol == OWNERSHIP_PROTOCOL
            && self.owner.len() == 32
            && self.owner.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}

/// An unknown additive extension must not hide readable legacy process identity.
/// Only the recognized shape participates in ownership recovery; all other
/// values still let the base record reach the conservative liveness probe.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OwnershipExtension {
    Protocol(OwnershipMetadata),
    Unknown(serde_json::Value),
}

impl OwnershipExtension {
    fn recognized(&self) -> Option<&OwnershipMetadata> {
        match self {
            Self::Protocol(metadata) if metadata.recognized() => Some(metadata),
            _ => None,
        }
    }
}

/// Keep the public LockInfo shape and the existing internal process record
/// readable by older consumers. Ownership metadata is an additive JSON field.
#[derive(Debug, Serialize, Deserialize)]
struct StoredRecord {
    #[serde(flatten)]
    record: LockRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ownership: Option<OwnershipExtension>,
}

#[derive(Debug)]
struct OsGuard {
    file: Option<File>,
}

impl OsGuard {
    fn release(&mut self) -> std::io::Result<()> {
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        // Command::spawn may briefly inherit the open file description before
        // exec closes it. End our ownership explicitly so that temporary copy
        // cannot extend a released reservation or invocation's admission.
        let result = file.unlock();
        drop(file); // Close remains the fallback even if explicit unlock fails.
        result
    }
}

impl Drop for OsGuard {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[derive(Debug)]
struct OwnedLock {
    // The guard is uncloneable; only its serialized lifecycle may release it.
    guard: OsGuard,
    owner: String,
}

/// An exclusive OS lock plus compatible, advisory JSON metadata.
///
/// The sidecar next to the JSON record is persistent and must not be unlinked.
/// Ownership lasts until release or drop, including while metadata is updated.
/// A released handle cannot update or remove a later holder's record.
#[derive(Debug)]
pub struct LockFile {
    pub(super) path: PathBuf,
    owned: Mutex<Option<OwnedLock>>,
}

/// Reserve ownership without replacing retained predecessor evidence.
///
/// Engine admission can inspect `prior_contents`, re-read retained artifacts,
/// and refuse before claiming. Dropping an unclaimed reservation unlocks and
/// closes its OS handle, leaving every predecessor JSON byte untouched.
#[derive(Debug)]
pub(crate) struct LockReservation {
    path: PathBuf,
    guard: OsGuard,
    prior: LockContents,
}

impl LockReservation {
    pub(crate) fn acquire(
        state_dir: &Path,
        workspace_root: Option<&Path>,
        timeout: Duration,
        force: bool,
    ) -> Result<Self> {
        Self::reserve(state_dir, workspace_root, Some((timeout, force)))
    }

    fn reserve(
        state_dir: &Path,
        workspace_root: Option<&Path>,
        recovery: Option<(Duration, bool)>,
    ) -> Result<Self> {
        fs::create_dir_all(state_dir)
            .with_context(|| format!("failed to create state dir {}", state_dir.display()))?;
        let path = lock_path(state_dir, workspace_root);
        let guard_path = guard_path(&path);
        let guard = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&guard_path)
            .with_context(|| format!("failed to open lock guard {}", guard_path.display()))?;
        match guard.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                // Metadata enriches diagnostics only; failure to read it never
                // changes the authoritative contention result.
                if let Ok(info) = read_lock_info_from_path(&path) {
                    bail!(
                        "lock already held by pid {} on {} since {} (plan_id: {:?}); OS ownership is active",
                        info.pid,
                        info.hostname,
                        info.acquired_at,
                        info.plan_id
                    );
                }
                bail!(
                    "lock already held: OS ownership is active at {}",
                    guard_path.display()
                );
            }
            Err(TryLockError::Error(error)) => {
                return Err(error).with_context(|| {
                    format!("failed to acquire OS lock guard {}", guard_path.display())
                });
            }
        }
        let guard = OsGuard { file: Some(guard) };

        let stored = match fs::read(&path) {
            Ok(bytes) => Some(parse_stored_record(&bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read lock file {}", path.display()));
            }
        };
        let prior = match stored {
            None => LockContents::Missing,
            Some(Ok(stored)) => {
                if let Some((timeout, force)) = recovery {
                    require_recoverable(&stored, timeout, force, Utc::now())?;
                }
                LockContents::Present(stored.record)
            }
            Some(Err(_)) => {
                if !matches!(recovery, Some((_, true))) {
                    bail!(
                        "failed to parse lock JSON from {}; retained metadata is preserved (explicit --force requires a quiescent legacy/foreign publisher)",
                        path.display()
                    );
                }
                LockContents::Corrupt
            }
        };
        let reservation = Self { path, guard, prior };
        if recovery.is_none()
            && let LockContents::Present(record) = reservation.prior_contents()
        {
            return Err(held_record_error(&record.info));
        }
        Ok(reservation)
    }

    pub(crate) fn prior_contents(&self) -> &LockContents {
        &self.prior
    }

    pub(crate) fn claim(self, plan_id: Option<&str>) -> Result<LockFile> {
        let owner = format!("{:032x}", rand::random::<u128>());
        let pid = std::process::id();
        let stored = StoredRecord {
            record: LockRecord {
                info: LockInfo {
                    pid,
                    hostname: local_hostname(),
                    acquired_at: Utc::now(),
                    plan_id: plan_id.map(str::to_owned),
                },
                process_identity: current_process_identity(pid),
            },
            ownership: Some(OwnershipExtension::Protocol(OwnershipMetadata {
                protocol: OWNERSHIP_PROTOCOL.to_owned(),
                owner: owner.clone(),
            })),
        };
        write_record(&self.path, &stored, &owner)?;
        Ok(LockFile {
            path: self.path,
            owned: Mutex::new(Some(OwnedLock {
                guard: self.guard,
                owner,
            })),
        })
    }
}

impl LockFile {
    /// Acquire exclusive ownership, refusing any retained JSON metadata.
    ///
    /// Use `acquire_with_timeout` for conservative abandoned-owner recovery.
    /// Checking `is_locked` before this call cannot establish ownership.
    pub fn acquire(state_dir: &Path, workspace_root: Option<&Path>) -> Result<Self> {
        LockReservation::reserve(state_dir, workspace_root, None)?.claim(None)
    }

    /// Acquire ownership and recover a provably abandoned predecessor.
    ///
    /// A same-host record from this sidecar protocol can be recovered after
    /// acquiring the OS guard. Legacy recovery additionally requires a matching
    /// local process scope, a positively absent process, and a nonnegative age
    /// strictly greater than `timeout`. Age alone never proves abandonment.
    /// Corrupt, foreign, and otherwise unknown metadata is preserved.
    pub fn acquire_with_timeout(
        state_dir: &Path,
        workspace_root: Option<&Path>,
        timeout: Duration,
    ) -> Result<Self> {
        Self::acquire_with_timeout_and_force(state_dir, workspace_root, timeout, false)
    }

    /// Acquire ownership with an explicit operator override for unknown metadata.
    ///
    /// `force` assumes any noncooperating legacy or foreign publisher is
    /// quiescent. It never overrides an actually held OS guard or a positively
    /// observed live legacy holder. Higher-level retained-state admission still
    /// applies independently; this method does not authorize another upload.
    pub fn acquire_with_timeout_and_force(
        state_dir: &Path,
        workspace_root: Option<&Path>,
        timeout: Duration,
        force: bool,
    ) -> Result<Self> {
        super::LockReservation::acquire(state_dir, workspace_root, timeout, force)?.claim(None)
    }

    /// Release this handle's ownership and remove only its matching JSON record.
    ///
    /// Repeated release is harmless. Missing or replaced metadata is preserved;
    /// neither this method nor Drop ever removes the persistent guard sidecar.
    pub fn release(&self) -> Result<()> {
        let mut slot = self
            .owned
            .lock()
            .map_err(|_| anyhow::anyhow!("lock lifecycle is poisoned"))?;
        let Some(mut owned) = slot.take() else {
            return Ok(());
        };
        let cleanup = match read_stored_record(&self.path) {
            Ok(Some(record)) if owns_record(&record, &owned.owner) => fs::remove_file(&self.path)
                .with_context(|| format!("failed to remove lock file {}", self.path.display())),
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        };
        // Keep the OS guard until after the ownership comparison and removal.
        // Even on metadata I/O failure, mark this handle released and explicitly
        // unlock it before close. The mutex still excludes this handle's callers.
        let unlock = owned
            .guard
            .release()
            .context("failed to release OS lock ownership");
        drop(owned);
        cleanup.and(unlock)
    }

    /// Update the plan while holding this invocation's OS guard and nonce.
    pub fn set_plan_id(&self, plan_id: &str) -> Result<()> {
        let slot = self
            .owned
            .lock()
            .map_err(|_| anyhow::anyhow!("lock lifecycle is poisoned"))?;
        let owned = slot
            .as_ref()
            .context("lock ownership has already been released")?;
        let mut stored = read_matching_record(&self.path, &owned.owner)?;
        stored.record.info.plan_id = Some(plan_id.to_owned());
        write_record(&self.path, &stored, &owned.owner)
    }

    /// Inspect this invocation's record without reacquiring its OS handle.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "guarded engine admission consumes this internal seam"
        )
    )]
    pub(crate) fn read_owned_record(&self) -> Result<LockRecord> {
        let slot = self
            .owned
            .lock()
            .map_err(|_| anyhow::anyhow!("lock lifecycle is poisoned"))?;
        let owned = slot
            .as_ref()
            .context("lock ownership has already been released")?;
        Ok(read_matching_record(&self.path, &owned.owner)?.record)
    }

    /// Report advisory JSON presence. This does not establish OS ownership.
    pub fn is_locked(state_dir: &Path, workspace_root: Option<&Path>) -> Result<bool> {
        Ok(lock_path(state_dir, workspace_root).exists())
    }

    /// Read compatible advisory holder metadata without taking ownership.
    pub fn read_lock_info(state_dir: &Path, workspace_root: Option<&Path>) -> Result<LockInfo> {
        read_lock_info_from_path(&lock_path(state_dir, workspace_root))
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn guard_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".guard");
    PathBuf::from(name)
}

fn local_hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

fn held_record_error(info: &LockInfo) -> anyhow::Error {
    anyhow::anyhow!(
        "lock already held by pid {} on {} since {} (plan_id: {:?})",
        info.pid,
        info.hostname,
        info.acquired_at,
        info.plan_id
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Predecessor {
    ReleasedProtocol,
    LiveLegacy,
    AbsentLegacy,
    Unknown,
}

fn observe_predecessor(stored: &StoredRecord) -> Predecessor {
    let record = &stored.record;
    if record.info.hostname != local_hostname() {
        return Predecessor::Unknown;
    }
    if stored
        .ownership
        .as_ref()
        .and_then(OwnershipExtension::recognized)
        .is_some()
    {
        // Called only after winning this key's OS guard. An old timestamp or
        // still-running PID cannot turn a released handle into an owner.
        return Predecessor::ReleasedProtocol;
    }
    let Some(expected) = &record.process_identity else {
        return Predecessor::Unknown;
    };
    let Ok(Some(scope)) = process_scope() else {
        return Predecessor::Unknown;
    };
    if expected.boot_id != scope.boot_id || expected.pid_namespace != scope.pid_namespace {
        return Predecessor::Unknown;
    }
    match process_identity(record.info.pid) {
        Ok(Some(actual)) if &actual == expected => Predecessor::LiveLegacy,
        Ok(None) if stored.ownership.is_none() => Predecessor::AbsentLegacy,
        _ => Predecessor::Unknown,
    }
}

fn recovery_allowed(
    predecessor: Predecessor,
    acquired_at: DateTime<Utc>,
    now: DateTime<Utc>,
    timeout: Duration,
    force: bool,
) -> bool {
    match predecessor {
        Predecessor::ReleasedProtocol => true,
        Predecessor::LiveLegacy => false,
        Predecessor::AbsentLegacy => {
            force
                || now
                    .signed_duration_since(acquired_at)
                    .to_std()
                    .is_ok_and(|age| age > timeout)
        }
        Predecessor::Unknown => force,
    }
}

fn require_recoverable(
    stored: &StoredRecord,
    timeout: Duration,
    force: bool,
    now: DateTime<Utc>,
) -> Result<()> {
    let predecessor = observe_predecessor(stored);
    ensure!(
        recovery_allowed(
            predecessor,
            stored.record.info.acquired_at,
            now,
            timeout,
            force
        ),
        "{}; retained owner is {predecessor:?}, so age does not authorize replacement",
        held_record_error(&stored.record.info)
    );
    Ok(())
}

fn owns_record(stored: &StoredRecord, owner: &str) -> bool {
    stored
        .ownership
        .as_ref()
        .and_then(OwnershipExtension::recognized)
        .is_some_and(|metadata| metadata.owner == owner)
}

fn read_stored_record(path: &Path) -> Result<Option<StoredRecord>> {
    match fs::read(path) {
        Ok(bytes) => Ok(parse_stored_record(&bytes).ok()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to read lock file {}", path.display()))
        }
    }
}

fn parse_stored_record(bytes: &[u8]) -> serde_json::Result<StoredRecord> {
    match serde_json::from_slice(bytes) {
        Ok(stored) => Ok(stored),
        Err(error) => {
            // Derive rejects duplicate recognized keys before it can expose
            // the base record. An additive ownership extension must never
            // hide otherwise readable live legacy identity from force policy.
            // Preserve that base, but do not infer protocol ownership or
            // authorize automatic legacy recovery from a damaged extension.
            match serde_json::from_slice::<LockRecord>(bytes) {
                Ok(record) => Ok(StoredRecord {
                    record,
                    ownership: Some(OwnershipExtension::Unknown(serde_json::Value::Null)),
                }),
                Err(_) => Err(error),
            }
        }
    }
}

fn read_matching_record(path: &Path, owner: &str) -> Result<StoredRecord> {
    let record = read_stored_record(path)?.context("lock metadata is missing or unreadable")?;
    ensure!(
        owns_record(&record, owner),
        "lock metadata no longer belongs to this owner"
    );
    Ok(record)
}

fn write_record(path: &Path, record: &StoredRecord, owner: &str) -> Result<()> {
    let json = serde_json::to_vec_pretty(record).context("failed to serialize lock info")?;
    let temporary = path.with_extension(format!("{owner}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("failed to create lock tmp file {}", temporary.display()))?;
    let result = (|| {
        file.write_all(&json)
            .context("failed to write lock metadata")?;
        file.sync_all().context("failed to sync lock metadata")?;
        drop(file);
        fs::rename(&temporary, path)
            .with_context(|| format!("failed to rename lock file to {}", path.display()))?;
        if let Some(parent) = path.parent()
            && let Ok(directory) = File::open(parent)
        {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests;
