# Layer: `plan` (planning algorithms)

**Position in the architecture:** Layer 4. Above `state/`, `runtime/`, `ops/`, below `engine/`.

## Single responsibility
Build a deterministic, topologically-ordered publish plan. Filter publishable crates, sort by dependency graph, group into parallel-eligible levels.

## Import rules
MAY import from `crate::state::*`, `crate::runtime::*`, `crate::ops::*`, `crate::types`, external crates.
MUST NOT import from `crate::engine::*`.
Enforced by `.github/workflows/architecture-guard.yml`.

## What lives here
- `plan/mod.rs` — main planning logic (currently the moved-but-unmodified `plan.rs`; will be replaced with absorbed `shipper-plan` content in a later PR once the `shipper-plan` absorption unblocks)
- `plan/levels/` — wave grouping for parallel publish (thin re-export from `shipper_types::levels`; the algorithm was absorbed from `shipper-levels` into `shipper-types` because `shipper_types::ReleasePlan::group_by_levels` is its primary consumer)
- `plan/chunking/` — future: was `shipper-chunking` (deferred — blocked by engine_parallel absorption)
