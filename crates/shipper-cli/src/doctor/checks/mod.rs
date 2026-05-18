//! Individual doctor checks, each focused on a single subsystem.
//!
//! Every check prints its own section to stdout and returns the
//! [`Finding`](super::findings::Finding) records it discovered (empty if
//! everything looks healthy).

pub(super) mod auth;
pub(super) mod connectivity;
pub(super) mod encryption;
pub(super) mod git;
pub(super) mod state_dir;
pub(super) mod tools;
