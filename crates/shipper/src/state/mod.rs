//! Layer 3: persistence. State, events, receipts, and the StateStore trait.
//!
//! May import from `runtime` and `ops`. Must not import from `engine` or `plan`.
//! See `CLAUDE.md` in this folder for the architectural rules.

pub mod events;
pub mod execution_state;
