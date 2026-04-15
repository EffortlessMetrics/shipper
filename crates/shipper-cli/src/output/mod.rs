//! CLI-specific output concerns: progress bars, formatting, reporters.
//!
//! These modules know about terminal capabilities. The library `shipper` crate
//! must not.

pub(crate) mod progress;
