//! Single-responsibility helpers for the sequential publish orchestrator.
//!
//! `engine::run_publish` remains the public entry point, while this module
//! owns the mechanically separate pieces around bootstrap, resume-gating, and
//! end-of-run finalization.

pub(super) mod bootstrap;
pub(super) mod finalize;
pub(super) mod resume;
