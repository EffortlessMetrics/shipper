# Module: `crate::state::execution_state`

**Layer:** state (layer 3)
**Single responsibility:** Persist `ExecutionState` and `Receipt` to disk (atomic write, durable rename, schema-versioned migration).
**Was:** standalone crate `shipper-state` (partial absorption in this PR)

## Public-to-crate API

- Schema version constants: `CURRENT_RECEIPT_VERSION`, `MINIMUM_SUPPORTED_VERSION`, `CURRENT_STATE_VERSION`, `CURRENT_PLAN_VERSION`
- File name constants: `STATE_FILE`, `RECEIPT_FILE`
- Path helpers: `state_path()`, `receipt_path()`
- Plaintext I/O: `load_state`, `save_state`, `clear_state`, `has_incomplete_state`, `load_receipt`, `write_receipt`, `fsync_parent_dir`
- Encrypted I/O: `load_state_encrypted`, `save_state_encrypted`, `load_receipt_encrypted`, `write_receipt_encrypted`
- Migration: `validate_receipt_version`, `migrate_receipt`

## Status

This module is the canonical path (`crate::state::execution_state::X`) that all
internal and CLI code now uses. The implementation currently re-exports from
the standalone `shipper-state` crate because `shipper-store` and
`shipper-engine-parallel` still depend on it via their own `shipper_state`
path-dep — changing that would require absorbing those crates too. When they
are absorbed in a future PR, the full implementation (currently at
`crates/shipper-state/src/lib.rs`) will move here and `shipper-state` will be
deleted from the workspace.

## Invariants

- Writes are atomic: write to a `.tmp` sibling, fsync, then rename.
- Forward-compatible schema: unknown receipt versions are still deserialised best-effort.
- v1 → v2 migration fills missing `git_context` (null) and `environment` fields and rewrites `receipt_version`.
