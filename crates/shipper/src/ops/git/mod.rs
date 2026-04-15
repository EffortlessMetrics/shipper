//! Git repository operations (cleanliness, context capture).
//!
//! Absorbed into `crate::ops::git` from the standalone `shipper-git` microcrate.
//! The facade is re-exported at `shipper::git` (see `lib.rs`) so external
//! consumers keep using the historical `shipper::git::*` public API.
//!
//! Split:
//!
//! - [`cleanliness`] — `is_git_clean` + `ensure_git_clean`, routing through
//!   [`bin_override`] when `SHIPPER_GIT_BIN` is set.
//! - [`context`] — commit/branch/tag/changed-files/remote queries + the
//!   [`collect_git_context`] aggregator.
//! - [`bin_override`] — parallel helpers that honor `SHIPPER_GIT_BIN`.
//!
//! See `CLAUDE.md` in this folder for architectural rules.

use std::env;

pub(crate) mod bin_override;
pub(crate) mod cleanliness;
pub(crate) mod context;

// Public-to-crate API re-exports; reachable via `shipper::git::*` through the
// façade module in `lib.rs`.
pub use cleanliness::{ensure_git_clean, is_git_clean};

use crate::types::GitContext;

/// Collect a [`GitContext`] for the current working directory.
///
/// Returns `None` when the CWD is not inside a git repository.
///
/// When `SHIPPER_GIT_BIN` is set, every sub-query is routed through the
/// `bin_override` helpers — there is no silent fallback to the default
/// `git` binary. Without the override, queries go through `context`.
pub fn collect_git_context() -> Option<GitContext> {
    let repo_root = std::env::current_dir().ok()?;

    let git_program = bin_override::git_program();
    if !bin_override::is_repo_root(&repo_root, &git_program) {
        return None;
    }

    if env::var("SHIPPER_GIT_BIN").is_ok() {
        let commit = bin_override::get_git_commit(&repo_root, &git_program);
        let branch = bin_override::get_git_branch(&repo_root, &git_program);
        let tag = bin_override::get_git_tag(&repo_root, &git_program);
        let dirty = bin_override::get_git_dirty_status(&repo_root, &git_program);
        return Some(GitContext {
            commit,
            branch,
            tag,
            dirty,
        });
    }

    if !context::is_git_repo(&repo_root) {
        return None;
    }

    Some(context::get_git_context(&repo_root))
}
