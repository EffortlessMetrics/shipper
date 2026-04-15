//! Append-only JSONL event log for publish operations.
//!
//! **Layer:** state (layer 3).
//!
//! This module is the absorbed home for event-log types. The implementation
//! currently lives in the standalone `shipper-events` crate because
//! `shipper-store` and `shipper-engine-parallel` still depend on it via their
//! own `shipper_events` path-dep. When those crates are absorbed in a future
//! PR, the full implementation (currently at `crates/shipper-events/src/lib.rs`)
//! will be moved here and `shipper-events` will be deleted.
//!
//! Items exposed:
//! - [`EventLog`] — in-memory append-only event log
//! - [`EVENTS_FILE`] — canonical event file name (`events.jsonl`)
//! - [`events_path`] — helper to build `<state_dir>/events.jsonl`

pub use shipper_events::{EVENTS_FILE, EventLog, events_path};
