//! # Shipper
//!
//! A reliability layer around `cargo publish` for Rust workspaces.
//!
//! Shipper provides deterministic, resumable crate publishing with comprehensive
//! safety checks and evidence collection. It makes `cargo publish` safe to start
//! and safe to re-run for multi-crate workspaces.
//!
//! ## Features
//!
//! - **Deterministic ordering** — Crates publish in a reproducible order based on the
//!   dependency graph, ensuring dependencies are always published before dependents.
//! - **Preflight verification** — Catch issues before publishing: git cleanliness,
//!   registry reachability, dry-run compilation, version existence, and ownership checks.
//! - **Readiness verification** — Confirm crates are visible on the registry after
//!   publishing, with configurable polling and backoff strategies.
//! - **Resumable execution** — Interrupted publishes can be resumed from where they
//!   left off, with state persisted to disk.
//! - **Evidence capture** — Receipts and event logs provide audit trails for every
//!   publish operation, including attempt counts, durations, and error classifications.
//! - **Parallel publishing** — Independent crates can be published concurrently for
//!   faster workspace releases (opt-in via [`types::ParallelConfig`]).
//! - **Multiple authentication methods** — Supports token-based authentication and
//!   GitHub Trusted Publishing.
//!
//! ## Pipeline
//!
//! The core flow is **plan → preflight → publish → (resume if interrupted)**:
//!
//! 1. [`plan::build_plan`] reads the workspace via `cargo_metadata`, filters
//!    publishable crates, and topologically sorts them.
//! 2. [`engine::run_preflight`] validates git cleanliness, registry
//!    reachability, dry-run, version existence, and optional ownership.
//! 3. [`engine::run_publish`] executes the plan with retry/backoff,
//!    verifying registry visibility after each crate.
//! 4. [`engine::run_resume`] reloads persisted state and continues from
//!    the first pending or failed package.
//!
//! ## Example
//!
//! ```ignore
//! use std::path::PathBuf;
//! use shipper::{plan, engine, types};
//!
//! // Build a publish plan from the workspace
//! let spec = types::ReleaseSpec {
//!     manifest_path: PathBuf::from("Cargo.toml"),
//!     registry: types::Registry::crates_io(),
//!     selected_packages: None,
//! };
//! let workspace = plan::build_plan(&spec)?;
//!
//! // Configure runtime options (all fields must be provided)
//! let opts = types::RuntimeOptions { /* ... */ };
//! ```
//!
//! ## Key Types
//!
//! - `ReleaseSpec` — Input specification (manifest path, registry, package filter)
//! - `ReleasePlan` — Deterministic, SHA256-identified publish plan
//! - `RuntimeOptions` — All runtime knobs (retry, readiness, policy, etc.)
//! - `Receipt` — Audit receipt with evidence for each published crate
//! - `PreflightReport` — Preflight assessment with finishability verdict
//! - `PublishPolicy` — Policy presets for safety vs. speed tradeoffs
//!
//! ## Modules
//!
//! - [`plan`] — Workspace analysis and topological plan generation
//! - [`engine`] — Core publish, preflight, and resume logic
//! - [`engine_parallel`] — Wave-based parallel publishing engine
//! - [`types`] — Domain types: specs, plans, options, receipts, errors
//! - [`config`] — Configuration file (`.shipper.toml`) loading and merging
//! - [`auth`] — Token resolution and authentication detection
//! - [`registry`] — Registry API and sparse-index client
//! - [`state`] — State and receipt persistence
//! - [`events`] — Append-only JSONL event log
//! - [`git`] — Git operations (cleanliness check, context capture)
//! - [`lock`] — Distributed lock to prevent concurrent publishes
//! - [`environment`] — Environment fingerprinting (OS, arch, tool versions)
//! - [`store`] — `StateStore` trait for pluggable persistence backends
//! - [`storage`] — Storage backends with pluggable `StorageBackend` trait
//! - [`cargo`] — Workspace metadata via `cargo_metadata`
//! - [`webhook`] — Webhook notifications for publish events
//!
//! ## Stability
//!
//! The library API is subject to change before v1.0.0. Breaking changes will be
//! documented in the [changelog](https://github.com/cmrigney/shipper/blob/main/CHANGELOG.md).
//!
//! ## CLI Usage
//!
//! For command-line usage, see the [shipper-cli crate](https://crates.io/crates/shipper-cli).

/// Token resolution: `CARGO_REGISTRY_TOKEN` → `CARGO_REGISTRIES_<NAME>_TOKEN`
/// → `$CARGO_HOME/credentials.toml`.
pub mod auth;

/// Workspace metadata via `cargo_metadata`.
pub mod cargo;

/// Configuration file (`.shipper.toml`) loading and merging.
pub mod config;

/// Core publish, preflight, and resume logic.
pub mod engine;

/// Wave-based parallel publishing engine.
pub mod engine_parallel;

/// Environment fingerprinting (OS, arch, tool versions).
pub mod environment;

/// Append-only JSONL event log.
pub mod events;

/// Git operations (cleanliness check, context capture).
pub mod git;

/// Distributed lock to prevent concurrent publishes.
/// Re-exported from shipper-lock microcrate.
pub use shipper_lock as lock;

/// Workspace analysis and topological plan generation.
pub mod plan;

/// Registry API and sparse-index client.
pub mod registry;

/// State and receipt persistence.
pub mod state;

/// `StateStore` trait for pluggable persistence backends.
pub mod store;

/// Storage backends with pluggable `StorageBackend` trait.
pub mod storage;

/// Domain types: specs, plans, options, receipts, errors.
pub mod types;

/// Configurable retry strategies with backoff and jitter.
/// Re-exported from shipper-retry microcrate.
pub use shipper_retry as retry;

/// Webhook notifications for publish events.
pub mod webhook;

/// State file encryption module.
/// Re-exported from shipper-encrypt microcrate.
pub use shipper_encrypt as encryption;

/// Property-based tests for shipper invariants.
#[cfg(test)]
mod property_tests;

/// Stress tests for concurrent operations.
#[cfg(test)]
mod stress_tests;
