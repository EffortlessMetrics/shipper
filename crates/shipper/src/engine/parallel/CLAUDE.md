# Module: `crate::engine::parallel`

**Layer:** engine (layer 5, top)

**Single responsibility:** Wave-based parallel publish engine. Schedules
independent crates into concurrent publish waves based on the dependency
graph produced by `shipper_plan::ReleasePlan::group_by_levels`.

**Was:** standalone crate `shipper-engine-parallel` (absorbed — chosen as
canonical because it had +1589 LOC of additional functionality vs the
in-tree duplicate, including the webhook submodule and BDD tests).

## Public API

- `pub fn run_publish_parallel(ws, opts, st, state_dir, reg, reporter)` —
  main entry point. Takes the host crate's `crate::registry::RegistryClient`
  and `crate::engine::Reporter` for seamless interop with `engine::mod`.
- `pub struct ParallelConfig` — re-exported through `shipper_types`.
- `pub trait Reporter` — local reporter trait used internally; the outer
  entry point adapts from `crate::engine::Reporter`.
- `pub use shipper_chunking::chunk_by_max_concurrent;` — re-export of the
  chunking helper used for wave planning.

## File layout

- `mod.rs` — public entry, `Reporter` trait, adapter, `run_publish_parallel`
  and its internal counterpart `run_publish_parallel_inner`, plus inline
  `#[cfg(test)] mod tests;` and `mod property_tests`.
- `publish.rs` — single-package/single-level primitives
  (`publish_package`, `run_publish_level`, `PackagePublishResult`).
- `readiness.rs` — readiness-visibility polling with backoff/jitter and
  sparse-index fallback.
- `policy.rs` — `policy_effects` adapter (translates `PublishPolicy` into
  resolved effects).
- `webhook.rs` — engine-specific webhook glue wrapping `shipper_webhook`.
  Kept as a sub-file because it defines a parallel-publish-specific
  `WebhookEvent` enum and payload builder that are not reusable outside
  this module.
- `tests.rs` — full test suite (unit, snapshot, policy, snapshot_tests
  submodule).
- `snapshots/` — insta snapshots for chunking, execution plans, policy
  effects.

## Invariants

- Topological wave ordering — crates within a wave have no inter-crate deps.
- All-or-nothing per wave: if any crate in a wave fails fatally, halt.
- State persisted after each crate completion (resumability).

## Internal microcrate dependencies (transitional)

This module currently imports from `shipper_execution_core`, `shipper_cargo`,
`shipper_events`, `shipper_plan`, `shipper_chunking`, `shipper_state`,
`shipper_registry`, `shipper_types`, `shipper_policy`, `shipper_webhook`,
`shipper_sparse_index`. As each of those microcrates is absorbed in
subsequent PRs, these imports will be rewritten to `crate::*` paths.
