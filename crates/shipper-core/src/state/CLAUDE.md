# Layer: `state` (persistence)

**Position in the architecture:** Layer 3. Above `runtime/` and `ops/`, below `plan/` and `engine/`.

## Single responsibility

Persist execution state, events, and receipts to disk (or other backends via the `StorageBackend` trait in `ops/storage/`). Provides resumable execution by reloading state.

## Import rules

`state` modules MAY import from `crate::runtime::*`, `crate::ops::*`, `crate::types`, external crates.
`state` modules MUST NOT import from `crate::engine::*` or `crate::plan::*`.
Enforced by `.github/workflows/architecture-guard.yml`.

## What lives here

- `state/execution_state/` — `ExecutionState`, receipt persistence, schema migration, and atomic filesystem I/O (was `shipper-state`)
- `state/events/` — Append-only JSONL event log (was `shipper-events`)
- `state/store/` — `StateStore` trait + filesystem impl (was `shipper-store`)

## File-sync contract

Ordinary event batches must flush and file-sync before projection commit. The
plaintext atomic JSON writer must propagate temporary-file sync errors before
rename. Parent-directory sync remains best effort; encrypted helper writes have
a separate implementation. `file_sync.rs` is a private shared syscall seam with
only `cfg(test)` path-scoped fault observation, not a public storage abstraction
or a runtime fault configuration. Preserve errors and already-written evidence.
See `docs/INVARIANTS.md` for the supported failure model and limits.
