# Module: `crate::runtime::execution`

**Layer:** runtime (layer 2)
**Single responsibility:** Shared execution primitives — backoff, error classification, package keys, state update locks.
**Was:** standalone crate `shipper-execution-core` (absorbed in this PR).

## Public-to-crate API

- `backoff_delay`
- `classify_cargo_failure`
- `pkg_key`
- `update_state_locked`
- `update_state`
- `resolve_state_dir`
- `short_state`

## Invariants

- Pure functions where possible.
- `update_state_locked`: caller must hold the appropriate lock before calling.
- `update_state`: mutates in-memory state, then persists; callers must tolerate the case where the in-memory mutation occurs even if the persist fails (known behavior, covered by tests).

## Internal microcrate dependencies (transitional)

This module currently imports from `crate::state::execution_state`,
`shipper_retry`, `shipper_types`, and `shipper_cargo_failure`. As each of the
remaining microcrates is absorbed in subsequent PRs, these imports will be
rewritten to `crate::*` paths.
