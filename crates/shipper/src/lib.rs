//! Install façade for the Shipper CLI and library.
//!
//! `shipper` is the user-facing crate. Install with
//! `cargo install shipper --locked` to get the CLI binary. Library
//! consumers can `use shipper::{plan, engine, ...}` exactly as before
//! — the public API is re-exported verbatim from the
//! [`shipper-core`](https://docs.rs/shipper-core) crate.
//!
//! Internally, `shipper` is a thin wrapper around two crates:
//!
//! - [`shipper-core`](https://docs.rs/shipper-core) — publishable library.
//! - [`shipper-cli`](https://docs.rs/shipper-cli) — CLI argument
//!   parsing and command dispatch; exposes `run()` which this crate's
//!   binary target calls.
//!
//! ## Why three crates?
//!
//! The split keeps seams clean: `shipper-core` publishes as a pure
//! library (no CLI/binary dependencies), `shipper-cli` publishes as
//! the CLI implementation, and this crate exists purely so
//! `cargo install shipper` does what users expect.

pub use shipper_core::*;
