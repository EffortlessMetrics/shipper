//! Layer 2: runtime context (pure data). Environment fingerprint, policy, execution context.
//!
//! May import from `ops`. Must not import from `engine`, `plan`, or `state`.
//! See `CLAUDE.md` in this folder for the architectural rules.

pub(crate) mod policy;
pub mod execution;
