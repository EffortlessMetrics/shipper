//! # Shipper
//!
//! Installable product face for the Shipper release engine.
//!
//! This is the crate you install with `cargo install shipper --locked`.
//! It ships the `shipper` binary, which delegates to the CLI adapter in
//! [`shipper-cli`](https://crates.io/crates/shipper-cli) — which in turn
//! calls the engine in [`shipper_core`].
//!
//! ## Architecture
//!
//! ```text
//! shipper (this crate — install façade)
//!   -> shipper-cli (CLI adapter: clap parsing, dispatch, output)
//!        -> shipper-core (engine: plan, preflight, publish, resume, …)
//! ```
//!
//! ## Install
//!
//! ```text
//! cargo install shipper --locked
//! ```
//!
//! ## Embedding
//!
//! For programmatic use without CLI dependencies (`clap`, `indicatif`),
//! depend on [`shipper-core`](https://crates.io/crates/shipper-core)
//! directly. This crate re-exports its public module surface below so
//! `shipper::engine::*`, `shipper::plan::*`, etc. keep resolving for
//! callers that prefer the product name — but new programmatic
//! consumers should target `shipper_core::*`.

pub use shipper_core::{
    auth, cargo, cargo_failure, config, encryption, engine, git, lock, plan, registry, retry,
    runtime, state, store, types, webhook,
};
