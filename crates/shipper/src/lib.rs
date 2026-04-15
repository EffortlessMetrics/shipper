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
//! - [`state`] — Layer 3 persistence: state, events, receipts
//! - [`git`] — Git operations (cleanliness check, context capture)
//! - [`lock`] — Distributed lock to prevent concurrent publishes
//! - [`environment`] — Environment fingerprinting (OS, arch, tool versions)
//! - [`store`] — `StateStore` trait for pluggable persistence backends
//! - [`storage`] — Storage backends with pluggable `StorageBackend` trait
//! - [`cargo`] — Workspace metadata via `cargo_metadata`
//! - [`cargo_failure`] — Cargo publish failure classification heuristics
//! - [`webhook`] — Webhook notifications for publish events
//!
//! ## Stability
//!
//! The library API is subject to change before v1.0.0. Breaking changes will be
//! documented in the [changelog](https://github.com/effortlessmetrics/shipper/blob/main/CHANGELOG.md).
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

/// Configuration file (`.shipper.toml`) loading and merging.
pub mod config;

/// Core publish, preflight, and resume logic.
pub mod engine;

/// Wave-based parallel publishing engine.
#[cfg(feature = "micro-parallel")]
#[path = "engine_parallel_micro.rs"]
pub mod engine_parallel;
#[cfg(not(feature = "micro-parallel"))]
pub mod engine_parallel;

/// Environment fingerprinting (OS, arch, tool versions).
#[cfg(feature = "micro-environment")]
#[path = "environment_micro.rs"]
pub mod environment;
#[cfg(not(feature = "micro-environment"))]
pub mod environment;

/// Git operations (cleanliness check, context capture).
#[cfg(feature = "micro-git")]
#[path = "git_micro.rs"]
pub mod git;
#[cfg(not(feature = "micro-git"))]
pub mod git;

/// Layer 1: I/O primitives (filesystem, git, cargo, OS, network).
///
/// Scaffolded in PR #49 and progressively populated as microcrates are
/// absorbed. See `crates/shipper/src/ops/CLAUDE.md` for layer rules.
pub(crate) mod ops;

/// Distributed lock to prevent concurrent publishes.
///
/// Re-export of [`crate::ops::lock`] (absorbed from the `shipper-lock`
/// microcrate during the decrating effort — see
/// `docs/decrating-plan.md` §6 Phase 2). The historical public path
/// `shipper::lock` is preserved for backward compatibility.
pub use crate::ops::lock;

/// Workspace analysis and topological plan generation.
#[cfg(feature = "micro-plan")]
#[path = "plan_micro.rs"]
pub mod plan;
#[cfg(not(feature = "micro-plan"))]
pub mod plan;

/// Cargo registry API and sparse-index client.
/// Re-exported from the [`shipper_registry`] crate (see [`crate::registry`]).
pub use shipper_registry as registry;

/// Layer 2: runtime context (pure data). Houses `runtime::policy`, etc.
pub(crate) mod runtime;

/// Layer 3: persistence. State, events, receipts.
pub mod state;

/// `StateStore` trait for pluggable persistence backends.
///
/// Absorbed from the former `shipper-store` microcrate. The implementation
/// now lives under `state/store/` to reflect the layered architecture;
/// the public path `shipper::store` is preserved for backward compatibility.
#[path = "state/store/mod.rs"]
pub mod store;

/// Storage backends with pluggable `StorageBackend` trait.
#[cfg(feature = "micro-storage")]
#[path = "storage_micro.rs"]
pub mod storage;
#[cfg(not(feature = "micro-storage"))]
pub mod storage;

/// Domain types: specs, plans, options, receipts, errors.
pub mod types;

/// Configurable retry strategies with backoff and jitter.
/// Re-exported from shipper-retry microcrate.
pub use shipper_retry as retry;

/// Cargo publish failure classification heuristics.
/// Re-exported from shipper-cargo-failure microcrate.
pub use shipper_cargo_failure as cargo_failure;

/// Webhook notifications for publish events.
pub mod webhook;

/// State file encryption module.
pub mod encryption;

/// Property-based tests for shipper invariants.
#[cfg(test)]
mod property_tests;

/// Stress tests for concurrent operations.
#[cfg(test)]
mod stress_tests;
