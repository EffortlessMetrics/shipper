# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

# Layer: `runtime` (runtime context — pure data)

**Position in the architecture:** Layer 2. Above `ops/`, below `state/`, `plan/`, `engine/`.

## Single responsibility

Pure-data descriptions of the runtime context: environment fingerprint, policy choices, execution context. No I/O, no side effects, no orchestration.

## Import rules

`runtime` modules MAY import from:
- `crate::ops::*` (the layer below)
- `crate::types` (re-exports of `shipper-types`)
- External pure-data crates (`serde`, `chrono`, etc.)

`runtime` modules MUST NOT import from:
- `crate::engine::...`
- `crate::plan::...`
- `crate::state::...`

These are enforced by `.github/workflows/architecture-guard.yml`.

## What lives here

- `runtime/environment/` — OS/arch/tool fingerprint (was `shipper-environment`)
- `runtime/policy/` — `PublishPolicy`, `VerifyMode`, `ReadinessMethod` enums (was `shipper-policy`)
- `runtime/execution/` — `ExecutionContext`, `ExecutionResult`, helpers (was `shipper-execution-core`)

## Boundary discipline

- Default visibility: `pub(crate)`.
- Each subfolder owns its own `mod.rs` and `CLAUDE.md`.
- Items here are mostly types and pure functions. If you find yourself doing I/O here, the code belongs in `ops/` instead.

