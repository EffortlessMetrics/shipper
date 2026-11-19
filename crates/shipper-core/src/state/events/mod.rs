//! Append-only JSONL event log for publish operations.
//!
//! **Layer:** state (layer 3).
//!
//! Absorbed from the former `shipper-events` microcrate (Phase 2 decrating).
//! The [`EventLog`] type stores publish lifecycle events in memory and can
//! persist them to disk as newline-delimited JSON (`.jsonl`).
//!
//! # JSONL format
//!
//! Each event is serialized as one JSON object per line using
//! [`shipper_types::PublishEvent`]. The output appends new events to existing
//! logs.
//!
//! # Ordering and time
//!
//! Insertion order, and then JSONL line order, is the authoritative replay
//! order. Callers record each package's causal sequence in that order; events
//! from different workers can interleave. The log does not sort or repair a
//! caller's sequence.
//!
//! Each event retains its original wall-clock observation timestamp. An event
//! can be created before another worker's event but appended later, and the
//! clock can move backward even within one package. Timestamps are neither
//! sequence numbers nor a reason to reorder events.
//!
//! The canonical file name for the event log is [`EVENTS_FILE`], resolved from
//! a state directory by [`events_path`].
//!
//! # Examples
//!
//! ## Append events and persist
//! ```ignore
//! use chrono::Utc;
//! use shipper::state::events::{EventLog, events_path};
//! use shipper_types::{EventType, PublishEvent};
//! use std::path::Path;
//!
//! let mut log = EventLog::new();
//! let event = PublishEvent {
//!     timestamp: Utc::now(),
//!     event_type: EventType::PackageStarted {
//!         name: "my-crate".to_string(),
//!         version: "1.0.0".to_string(),
//!     },
//!     package: "my-crate@1.0.0".to_string(),
//! };
//!
//! log.record(event);
//! let path = events_path(Path::new(".shipper"));
//! log.write_to_file(&path).expect("write events");
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use shipper_types::PublishEvent;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;

/// Canonical event file name.
pub const EVENTS_FILE: &str = "events.jsonl";

/// Canonical file name for a session-isolated preflight audit (#100).
///
/// Used by `shipper preflight --preflight-only` to keep a fresh
/// finishability audit from appending into the authoritative
/// `events.jsonl` log, preserving events-as-truth for the publish
/// flow while still producing an auditable JSONL trace for the
/// standalone preflight run.
pub const PREFLIGHT_ONLY_EVENTS_FILE_PREFIX: &str = "preflight-only-";
pub const PREFLIGHT_ONLY_EVENTS_FILE_SUFFIX: &str = ".events.jsonl";

/// Get the events file path for a state directory.
///
/// The returned value is always `state_dir/events.jsonl`.
pub fn events_path(state_dir: &Path) -> PathBuf {
    state_dir.join(EVENTS_FILE)
}

/// Get the session-isolated preflight audit events file path (#100).
///
/// Used by `shipper preflight --preflight-only` so that a fresh audit
/// never appends to the authoritative `events.jsonl` log. Each
/// invocation writes to its own session-scoped JSONL file, keeping
/// standalone audits isolated from both publish history and one another.
pub fn preflight_only_events_path(state_dir: &Path, session_id: &str) -> PathBuf {
    state_dir.join(format!(
        "{PREFLIGHT_ONLY_EVENTS_FILE_PREFIX}{session_id}{PREFLIGHT_ONLY_EVENTS_FILE_SUFFIX}"
    ))
}

/// Return all preflight-only event sidecars in lexical order.
pub fn preflight_only_events_paths(state_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();

    if !state_dir.exists() {
        return Ok(paths);
    }

    for entry in fs::read_dir(state_dir)
        .with_context(|| format!("failed to read state dir {}", state_dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", state_dir.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", entry.path().display()))?;

        if !file_type.is_file() {
            continue;
        }

        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };

        if file_name.starts_with(PREFLIGHT_ONLY_EVENTS_FILE_PREFIX)
            && file_name.ends_with(PREFLIGHT_ONLY_EVENTS_FILE_SUFFIX)
        {
            paths.push(entry.path());
        }
    }

    paths.sort();
    Ok(paths)
}

/// Append-only event log for publish operations.
///
/// Events are stored in-memory in insertion order, preserving their supplied
/// timestamps even when those timestamps decrease.
///
/// An indeterminate append, flush, or file-sync failure stops further writes
/// through this instance and retains its pending events, including across
/// [`Self::clear`].
#[derive(Debug, Default)]
pub struct EventLog {
    events: Vec<PublishEvent>,
    failed_write: Mutex<Option<EventWriteFailure>>,
}

/// Retain diagnostics without replacing the first returned error. Later
/// refusals reconstruct its I/O metadata because `io::Error` is not Clone;
/// arbitrary custom source types are only preserved in the first error.
#[derive(Debug)]
struct EventWriteFailure {
    diagnostic: String,
    io: Option<EventIoFailure>,
}

#[derive(Debug)]
struct EventIoFailure {
    kind: io::ErrorKind,
    raw_os_error: Option<i32>,
    message: String,
}

impl EventWriteFailure {
    fn capture(error: &anyhow::Error) -> Self {
        let io = error.chain().find_map(|source| {
            source
                .downcast_ref::<io::Error>()
                .map(|error| EventIoFailure {
                    kind: error.kind(),
                    raw_os_error: error.raw_os_error(),
                    message: error.to_string(),
                })
        });
        Self {
            diagnostic: format!("{error:#}"),
            io,
        }
    }

    fn refusal(&self) -> anyhow::Error {
        let context = format!(
            "event log writes stopped after an earlier persistence failure; pending events retained: {}",
            self.diagnostic
        );
        match &self.io {
            Some(error) => {
                let source = match error.raw_os_error {
                    Some(code) => io::Error::from_raw_os_error(code),
                    None => io::Error::new(error.kind, error.message.clone()),
                };
                anyhow::Error::new(source).context(context)
            }
            None => anyhow::anyhow!(context),
        }
    }
}

impl EventLog {
    /// Create a new empty event log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new event.
    ///
    /// Added events are appended and remain in order. The supplied observation
    /// timestamp is preserved; it is not replaced by the time of this call.
    pub fn record(&mut self, event: PublishEvent) {
        self.events.push(event);
    }

    /// Write all recorded events to a file in JSONL format.
    ///
    /// The file is opened in append mode and existing contents are preserved.
    /// Recorded events are written in insertion order without sorting by time.
    /// Success requires both flushing the buffered JSONL and synchronizing the
    /// underlying file. A returned error can leave complete or partial appended
    /// bytes. Once an append has been attempted, any error stops later writes
    /// through this instance, even to another path or after [`Self::clear`].
    /// Pending evidence and the original diagnostic remain available. Errors
    /// creating directories or opening the file precede the append and may be
    /// retried. This hold does not survive constructing a new log or restarting
    /// the process. Parent-directory persistence is not guaranteed.
    pub fn write_to_file(&self, path: &Path) -> Result<()> {
        // Serialize attempts even for callers sharing an immutable EventLog.
        // The engine also has an outer owner lock, but that is not required by
        // this public API's failed-batch contract.
        let mut failed_write = self.failed_write.lock().map_err(|_| {
            anyhow::anyhow!("event log write owner is poisoned; pending events retained")
        })?;
        if let Some(failure) = failed_write.as_ref() {
            return Err(failure.refusal());
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create events dir {}", parent.display()))?;
        }

        // Append mode: open file, write new events
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("failed to open events file {}", path.display()))?;

        let mut writer = std::io::BufWriter::new(file);
        let result = (|| {
            #[cfg(test)]
            super::file_sync::test_support::before_io(
                super::file_sync::test_support::Point::EventAppend,
                path,
            )
            .with_context(|| format!("failed to append events file {}", path.display()))?;
            self.write_events_to(&mut writer)
                .with_context(|| format!("failed to append events file {}", path.display()))?;
            writer
                .flush()
                .with_context(|| format!("failed to flush events file {}", path.display()))?;
            super::file_sync::sync_all(writer.get_ref(), path)
                .with_context(|| format!("failed to sync events file {}", path.display()))?;
            Ok(())
        })();
        if let Err(error) = &result {
            *failed_write = Some(EventWriteFailure::capture(error));
        }
        // Do not let BufWriter::drop attempt another flush after a failed
        // append. The complete logical batch remains in self.events.
        let _ = writer.into_parts();
        result
    }

    fn write_events_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        for event in &self.events {
            serde_json::to_writer(&mut *writer, event)
                .context("failed to serialize event to JSON")?;
            writer
                .write_all(b"\n")
                .context("failed to write event line")?;
        }

        Ok(())
    }

    /// Read all events from a JSONL file.
    ///
    /// Returns an empty log when the file does not exist. Events retain file
    /// order and their recorded timestamp values, including clock rollback.
    pub fn read_from_file(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let file = File::open(path)
            .with_context(|| format!("failed to open events file {}", path.display()))?;

        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for line in reader.lines() {
            let line = line.with_context(|| {
                format!("failed to read line from events file {}", path.display())
            })?;
            let event: PublishEvent = serde_json::from_str(&line)
                .with_context(|| format!("failed to parse event JSON from line: {}", line))?;
            events.push(event);
        }

        Ok(Self {
            events,
            ..Self::default()
        })
    }

    /// Get all events for a specific package.
    ///
    /// Matching is exact against the `package` field and preserves the matching
    /// events' relative append order.
    pub fn events_for_package(&self, package: &str) -> Vec<&PublishEvent> {
        self.events
            .iter()
            .filter(|e| e.package == package)
            .collect()
    }

    /// Get all recorded events in append order, without timestamp sorting.
    pub fn all_events(&self) -> &[PublishEvent] {
        &self.events
    }

    /// Clear recorded events when this instance has no indeterminate write.
    ///
    /// A failed-write or poisoned owner retains all pending evidence and stays
    /// unwritable; clearing memory cannot establish which bytes were appended.
    pub fn clear(&mut self) {
        if self
            .failed_write
            .get_mut()
            .is_ok_and(|failure| failure.is_none())
        {
            self.events.clear();
        }
    }

    /// Get the number of recorded events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Check if the log is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
