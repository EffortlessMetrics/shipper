//! Cross-platform command execution with optional timeout support.
//!
//! This module is the I/O primitive other `shipper` subsystems (notably
//! [`crate::cargo`]) use to shell out to `cargo publish`, `cargo package`,
//! and similar commands.
//!
//! Absorbed from the standalone `shipper-process` crate during the Phase 2
//! decrating effort. See `docs/decrating-plan.md` §6.

mod capture;
mod cargo;
pub(crate) mod interruption;
mod run;
mod timeout;
mod types;
mod which;

// Production callers use complete `run_command_with_timeout` or the explicit
// bounded `run_command_with_publish_capture` path; other
// helpers are retained for the tests that were co-located with the
// original microcrate (see `tests.rs`, `snapshot_tests.rs`,
// `cross_platform_edge_case_tests.rs`).
pub(crate) use self::capture::{
    CaptureLoss, CaptureStatus, CapturedCommandOutput, CleanupFailure, PUBLISH_TAIL_BYTES,
};
#[allow(unused_imports)]
pub(crate) use self::cargo::{cargo_dry_run, cargo_publish, run_cargo, run_cargo_in_dir};
#[allow(unused_imports)]
pub(crate) use self::run::{
    run_command, run_command_in_dir, run_command_simple, run_command_streaming,
    run_command_with_env,
};
#[allow(unused_imports)]
pub(crate) use self::timeout::run_command_with_timeout;
pub(crate) use self::timeout::{CommandNotLaunched, run_command_with_publish_capture};
#[allow(unused_imports)]
pub(crate) use self::types::{CommandOutput, CommandResult};
#[allow(unused_imports)]
pub(crate) use self::which::{command_exists, which};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod cross_platform_edge_case_tests;
