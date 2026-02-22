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
//! - [`process`] — Cross-platform command execution with timeout support
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
#[cfg(feature = "micro-auth")]
#[path = "auth_micro.rs"]
pub mod auth;
#[cfg(not(feature = "micro-auth"))]
pub mod auth;

/// Workspace metadata and publish execution via cargo.
#[cfg(feature = "micro-cargo")]
#[path = "cargo_micro.rs"]
pub mod cargo;
#[cfg(not(feature = "micro-cargo"))]
pub mod cargo;

/// Process execution with optional timeout support.
#[cfg(feature = "micro-process")]
#[path = "process_micro.rs"]
pub mod process;
#[cfg(not(feature = "micro-process"))]
pub mod process;

/// Configuration file (`.shipper.toml`) loading and merging.
#[cfg(feature = "micro-config")]
#[path = "config_micro.rs"]
pub mod config;
#[cfg(not(feature = "micro-config"))]
pub mod config;

/// Core publish, preflight, and resume logic.
pub mod engine;

/// Wave-based parallel publishing engine.
pub mod engine_parallel;

/// Environment fingerprinting (OS, arch, tool versions).
#[cfg(feature = "micro-environment")]
#[path = "environment_micro.rs"]
pub mod environment;
#[cfg(not(feature = "micro-environment"))]
pub mod environment;

/// Append-only JSONL event log.
#[cfg(feature = "micro-events")]
#[path = "events_micro.rs"]
pub mod events;
#[cfg(not(feature = "micro-events"))]
pub mod events;

/// Git operations (cleanliness check, context capture).
#[cfg(feature = "micro-git")]
#[path = "git_micro.rs"]
pub mod git;
#[cfg(not(feature = "micro-git"))]
pub mod git;

/// Distributed lock to prevent concurrent publishes.
#[cfg(feature = "micro-lock")]
#[path = "lock_micro.rs"]
pub mod lock;
#[cfg(not(feature = "micro-lock"))]
pub mod lock;

/// Workspace analysis and topological plan generation.
pub mod plan;

/// Registry API and sparse-index client.
#[cfg(feature = "micro-registry")]
#[path = "registry_micro.rs"]
pub mod registry;
#[cfg(not(feature = "micro-registry"))]
pub mod registry;

/// State and receipt persistence.
#[cfg(feature = "micro-state")]
#[path = "state_micro.rs"]
pub mod state;
#[cfg(not(feature = "micro-state"))]
pub mod state;

/// `StateStore` trait for pluggable persistence backends.
#[cfg(feature = "micro-store")]
#[path = "store_micro.rs"]
pub mod store;
#[cfg(not(feature = "micro-store"))]
pub mod store;

/// Storage backends with pluggable `StorageBackend` trait.
#[cfg(feature = "micro-storage")]
#[path = "storage_micro.rs"]
pub mod storage;
#[cfg(not(feature = "micro-storage"))]
pub mod storage;

/// Domain types: specs, plans, options, receipts, errors.
#[cfg(feature = "micro-types")]
#[path = "types_micro.rs"]
pub mod types;
#[cfg(not(feature = "micro-types"))]
pub mod types;

/// Configurable retry strategies with backoff and jitter.
/// Re-exported from shipper-retry microcrate.
pub use shipper_retry as retry;

/// Webhook notifications for publish events.
#[cfg(feature = "micro-webhook")]
#[path = "webhook_micro.rs"]
pub mod webhook;
#[cfg(not(feature = "micro-webhook"))]
pub mod webhook;

/// State file encryption module.
#[cfg(feature = "micro-encrypt")]
#[path = "encryption_micro.rs"]
pub mod encryption;
#[cfg(not(feature = "micro-encrypt"))]
pub mod encryption;

/// Property-based tests for shipper invariants.
#[cfg(test)]
mod property_tests;

/// Stress tests for concurrent operations.
#[cfg(test)]
mod stress_tests;
