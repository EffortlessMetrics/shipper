//! Layer 1: I/O primitives. Talk to the filesystem, git, cargo, OS, network.
//!
//! This layer must not import from `engine`, `plan`, `state`, or `runtime`.
//! See `CLAUDE.md` in this folder for the architectural rules.
//!
//! Modules here were previously standalone microcrates (e.g. `shipper-auth`)
//! and have been pulled into the core `shipper` crate as crate-private
//! modules. External consumers can still reach the public surface via
//! re-exports at the crate root (e.g. `pub use crate::ops::auth;` in
//! `lib.rs`).

pub(crate) mod auth;
pub mod cargo;
pub(crate) mod git;
pub mod lock;
pub(crate) mod process;

// Storage trait + filesystem impl; kept as `pub(crate)` even though no
// internal callers exist yet, because future persistence backends will
// plug in here. `dead_code` is allowed so this scaffolding doesn't block
// `-D warnings` builds.
#[allow(dead_code)]
pub(crate) mod storage;
