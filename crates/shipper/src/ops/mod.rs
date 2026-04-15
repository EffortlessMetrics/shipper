//! Layer 1: I/O primitives. Talk to the filesystem, git, cargo, OS, network.
//!
//! This layer must not import from `engine`, `plan`, `state`, or `runtime`.
//! See `CLAUDE.md` in this folder for the architectural rules.

// Subsystem modules are added here as they are absorbed from microcrates.
// `shipper::lock` is re-exported from `crate::ops::lock` in `lib.rs` to
// preserve the historical public API surface.
pub mod lock;
pub(crate) mod process;
