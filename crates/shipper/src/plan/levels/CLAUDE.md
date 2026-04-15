# Module: `crate::plan::levels`

**Layer:** plan (layer 4)
**Single responsibility:** Group publishable crates into parallel-eligible "waves" — crates within a wave have no dependencies on each other and can publish concurrently.
**Was:** standalone crate `shipper-levels` (absorbed in this PR)

## Public-to-crate API
- `PublishLevel<T>` — generic wave struct (level number + packages).
- `group_packages_by_levels` — Kahn-style topological leveling that falls back to deterministic singletons on cycles.

Both items are re-exported here from `shipper_types::levels`, which is where the algorithm's source of truth lives. `shipper-types` owns the algorithm because its `ReleasePlan::group_by_levels` method is the primary in-source consumer and `shipper-types` can't depend on `shipper`.

## Invariants
- Levels are computed deterministically (BTreeSet ordering) — same input produces the same level grouping.
- A crate's level = max(level of any in-plan dep) + 1.
- Dependencies outside the plan are ignored for leveling.
- Cycles are drained one package per level in deterministic order.
