# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

# Layer: `engine` (orchestration — top of the stack)

**Position in the architecture:** Layer 5 (top). Coordinates all lower layers.

## Single responsibility

Orchestrate the **plan -> preflight -> publish -> resume** pipeline. Loop
through the plan, invoke registry/cargo operations, persist state after each
step, retry on transient failures, classify errors.

## Import rules

`engine` modules MAY import from any layer below: `crate::plan::*`,
`crate::state::*`, `crate::runtime::*`, `crate::ops::*`, `crate::types`, plus
public crates (`shipper_registry`, `shipper_webhook`, `shipper_retry`, etc.).

`engine` is the top of the dependency tree — nothing imports from `engine`
except `lib.rs` re-exports and `shipper-cli`.

## What lives here

- `engine/mod.rs` — current orchestration entry points (`run_preflight`,
  `run_publish`, `run_resume`) and the `Reporter` trait. This file was moved
  verbatim from `crates/shipper/src/engine.rs` when the `engine/` layer dir
  was introduced.
- `engine/parallel/` — wave-based parallel publish (was the standalone
  `shipper-engine-parallel` crate, absorbed in the same PR that created this
  layer dir).
- Future: `engine/preflight/`, `engine/publish/`, `engine/resume/`,
  `engine/readiness/` as `engine/mod.rs` gets split up.

