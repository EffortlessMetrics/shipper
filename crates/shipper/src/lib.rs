//! Shipper: a publishing reliability layer for Rust workspaces.
//!
//! The library is structured around:
//! - building a deterministic [`ReleasePlan`]
//! - running preflight checks
//! - executing the plan with persistence and retry/backoff

pub mod auth;
pub mod cargo;
pub mod engine;
pub mod git;
pub mod plan;
pub mod registry;
pub mod state;
pub mod types;
