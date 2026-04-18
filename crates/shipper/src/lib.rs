//! # Shipper
//!
//! Installable product face for the Shipper release engine.
//!
//! This crate is what users `cargo install`. The engine itself lives in
//! [`shipper_core`]; the `shipper` binary wires [`shipper_core`] to a
//! command-line adapter (currently [`cli`], migrating to the
//! `shipper-cli` crate in the #95 split).
//!
//! For programmatic use without a CLI dependency graph, depend on
//! [`shipper-core`](https://crates.io/crates/shipper-core) directly.
//!
//! ## Install
//!
//! ```text
//! cargo install shipper --locked
//! ```
//!
//! ## Re-exports
//!
//! Every public module of [`shipper_core`] is re-exported here so
//! existing `shipper::engine`, `shipper::plan`, etc. paths keep
//! resolving during the #95 migration. New programmatic consumers
//! should prefer `shipper_core::*` directly.

pub use shipper_core::{
    auth, cargo, cargo_failure, config, encryption, engine, git, lock, plan, registry, retry,
    runtime, state, store, types, webhook,
};

/// CLI entry point for the `shipper` binary.
///
/// Currently colocated in the `shipper` crate; moves to `shipper-cli`
/// as a library adapter in #95 PR 2. The binary target at
/// `src/bin/shipper.rs` is a three-line shim over [`cli::run`].
pub mod cli;
