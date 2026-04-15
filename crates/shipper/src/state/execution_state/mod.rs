//! Execution state and receipt persistence (atomic write + schema-versioned migration).
//!
//! **Layer:** state (layer 3).
//!
//! This module is the absorbed home for execution-state types. The implementation
//! currently lives in the standalone `shipper-state` crate because
//! `shipper-store` and `shipper-engine-parallel` still depend on it via their
//! own `shipper_state` path-dep. When those crates are absorbed in a future
//! PR, the full implementation (currently at `crates/shipper-state/src/lib.rs`)
//! will be moved here and `shipper-state` will be deleted.

pub use shipper_state::*;
