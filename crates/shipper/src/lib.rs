//! Shipper: a publishing reliability layer for Rust workspaces.
//!
//! Shipper makes `cargo publish` safe to start and safe to re-run for
//! multi-crate workspaces. It handles dependency ordering, preflight
//! verification, retry with backoff, readiness checking, and resumable
//! state persistence.
//!
//! # Pipeline
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
//! # Key types
//!
//! - [`types::ReleaseSpec`] — input specification (manifest path, registry, package filter)
//! - [`types::ReleasePlan`] — deterministic, SHA256-identified publish plan
//! - [`types::RuntimeOptions`] — all runtime knobs (retry, readiness, policy, etc.)
//! - [`types::Receipt`] — audit receipt with evidence for each published crate
//! - [`types::PreflightReport`] — preflight assessment with finishability verdict

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
pub mod lock;

/// Workspace analysis and topological plan generation.
pub mod plan;

/// Registry API and sparse-index client.
pub mod registry;

/// State and receipt persistence.
pub mod state;

/// `StateStore` trait for pluggable persistence backends.
pub mod store;

/// Domain types: specs, plans, options, receipts, errors.
pub mod types;
