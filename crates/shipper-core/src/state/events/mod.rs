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
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

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
/// Events are stored in-memory in insertion order.
#[derive(Debug, Default)]
pub struct EventLog {
    events: Vec<PublishEvent>,
}

impl EventLog {
    /// Create a new empty event log.
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Record a new event.
    ///
    /// Added events are appended and remain in order.
    pub fn record(&mut self, event: PublishEvent) {
        self.events.push(event);
    }

    /// Write all recorded events to a file in JSONL format.
    ///
    /// The file is opened in append mode and existing contents are preserved.
    pub fn write_to_file(&self, path: &Path) -> Result<()> {
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

        self.write_events_to(&mut writer)?;

        writer.flush().context("failed to flush events file")?;

        Ok(())
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
    /// Returns an empty log when the file does not exist.
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

        Ok(Self { events })
    }

    /// Get all events for a specific package.
    ///
    /// Matching is exact against the `package` field.
    pub fn events_for_package(&self, package: &str) -> Vec<&PublishEvent> {
        self.events
            .iter()
            .filter(|e| e.package == package)
            .collect()
    }

    /// Get all recorded events.
    pub fn all_events(&self) -> &[PublishEvent] {
        &self.events
    }

    /// Clear all recorded events from memory.
    pub fn clear(&mut self) {
        self.events.clear();
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
