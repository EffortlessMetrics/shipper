//! Layer 2: runtime context (pure data). Environment fingerprint, policy, execution context.
//!
//! May import from `ops`. Must not import from `engine`, `plan`, or `state`.
//! See `CLAUDE.md` in this folder for the architectural rules.

pub mod execution;
pub(crate) mod policy;

// Some absorbed `environment` items (CI branch/SHA/PR helpers, pipe-fingerprint
// form, `normalize_tool_version`, `EnvironmentInfo::fingerprint`) currently
// have no in-crate callers but have full test coverage. They were public API
// of the former `shipper-environment` microcrate and are kept available for
// future wiring (e.g., webhook/event metadata) rather than dropped.
#[allow(dead_code)]
pub(crate) mod environment;
