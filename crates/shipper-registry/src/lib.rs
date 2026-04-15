//! Registry API client for shipper.
//!
//! This crate provides API clients for interacting with crate registries
//! like crates.io, supporting version existence checks, sparse-index visibility
//! polling, ownership verification, and readiness backoff loops.
//!
//! # Modules
//!
//! - [`context`] — the primary [`RegistryClient`] that is `Registry`-aware.
//!   Accepts a full [`shipper_types::Registry`] (name, api_base, index_base)
//!   and exposes the complete registry surface used by the shipper library:
//!   API existence checks, sparse-index visibility, owner queries,
//!   `is_version_visible_with_backoff` with exponential-backoff evidence, etc.
//! - [`http`] — a lightweight HTTP client [`http::HttpRegistryClient`] that
//!   takes a bare base-URL string. Intended for callers that do not need the
//!   full `Registry` context (e.g. the parallel engine helper crate).
//!
//! # Example
//!
//! ```no_run
//! use shipper_registry::RegistryClient;
//! use shipper_types::Registry;
//!
//! let registry = Registry::crates_io();
//! let client = RegistryClient::new(registry).expect("client");
//! let visible = client.version_exists("serde", "1.0.0").unwrap_or(false);
//! ```

pub mod context;
pub mod http;

// Primary public API: the canonical, Registry-aware client.
pub use context::{Owner, OwnersResponse, RegistryClient};

// Lightweight HTTP client for callers that only have a base URL.
pub use http::HttpRegistryClient;

// Additional types useful to external callers.
pub use http::{CrateInfo, OwnersApiUser};

/// Default API endpoint for crates.io
pub const CRATES_IO_API: &str = "https://crates.io";

/// Default timeout for API requests (used by [`HttpRegistryClient`]).
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Default user agent for API requests.
pub const USER_AGENT: &str = concat!("shipper/", env!("CARGO_PKG_VERSION"));

/// Compute the sparse-index path for a crate name.
pub fn sparse_index_path(crate_name: &str) -> String {
    shipper_sparse_index::sparse_index_path(crate_name)
}

/// Check if a crate version is visible on the registry via its API.
///
/// Convenience wrapper that constructs an [`HttpRegistryClient`] and calls
/// [`HttpRegistryClient::version_exists`].
pub fn is_version_visible(base_url: &str, name: &str, version: &str) -> anyhow::Result<bool> {
    let client = HttpRegistryClient::new(base_url);
    client.version_exists(name, version)
}

/// Check if a crate exists on the registry via its API.
///
/// Convenience wrapper that constructs an [`HttpRegistryClient`] and calls
/// [`HttpRegistryClient::crate_exists`].
pub fn is_crate_visible(base_url: &str, name: &str) -> anyhow::Result<bool> {
    let client = HttpRegistryClient::new(base_url);
    client.crate_exists(name)
}
