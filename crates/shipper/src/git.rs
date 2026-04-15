//! Public façade for git operations.
//!
//! Re-exports the absorbed [`crate::ops::git`] module's public API so external
//! consumers (notably `shipper-cli`) keep using `shipper::git::*` after the
//! `shipper-git` microcrate absorption.
//!
//! See `crates/shipper/src/ops/git/CLAUDE.md` for architectural notes.

pub use crate::ops::git::{collect_git_context, ensure_git_clean, is_git_clean};
