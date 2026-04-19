//! # Shipper
//!
//! Installable product face for the Shipper release engine.
//!
//! This is the crate you install with `cargo install shipper --locked`.
//! It ships the `shipper` binary, which delegates to the CLI adapter in
//! [`shipper-cli`](https://crates.io/crates/shipper-cli) — which in turn
//! calls the engine in [`shipper-core`](https://crates.io/crates/shipper-core).
//!
//! ## Architecture
//!
//! ```text
//! shipper (this crate — install façade, curated re-export)
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
//! For programmatic use — driving a publish from your own Rust code
//! without a CLI dependency graph (no `clap`, no `indicatif`) — you
//! have two options:
//!
//! 1. Depend on [`shipper-core`](https://crates.io/crates/shipper-core)
//!    directly. That crate is the stable embedding surface.
//! 2. Or disable the default `cli` feature on this crate:
//!
//!    ```toml
//!    shipper = { version = "...", default-features = false }
//!    ```
//!
//!    This drops the `shipper-cli` (and therefore `clap`) dependency
//!    while keeping the curated re-export paths below.
//!
//! This crate re-exports a **curated** set of `shipper-core` modules
//! for convenience so `shipper::engine`, `shipper::plan`, etc. keep
//! resolving for drivers that prefer the product name:
//!
//! - [`engine`] — preflight, publish, resume, rehearsal
//! - [`plan`] — build a deterministic publish plan
//! - [`types`] — domain types (specs, receipts, state, events)
//! - [`config`] — load and merge `.shipper.toml`
//! - [`state`] — read persisted execution state and events
//! - [`store`] — the `StateStore` trait and filesystem implementation
//!
//! Engine internals (`auth`, `cargo`, `encryption`, `git`, `lock`,
//! `registry`, `retry`, `runtime`, `webhook`, `cargo_failure`) are
//! intentionally not re-exported here. Reach for them through
//! `shipper-core` directly — that's a signal you're embedding, not
//! driving.

pub use shipper_core::{config, engine, plan, state, store, types};
