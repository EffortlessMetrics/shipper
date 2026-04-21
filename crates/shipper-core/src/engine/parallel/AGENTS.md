# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

## Purpose

This directory handles wave-based parallel publishing for crates that can be released concurrently.

## Key files

- `mod.rs` — entrypoint, reporter adapter, and top-level coordination.
- `publish.rs` — per-package and per-wave execution.
- `readiness.rs` and `reconcile.rs` — visibility checks and ambiguous publish handling.
- `tests.rs` and `snapshots/` — scheduling and behavior coverage.

## Invariants

- Waves must still respect dependency order.
- Persist state after package completion so interrupted runs stay resumable.
- Keep retry, readiness, and reconciliation behavior aligned with the main engine contracts.

## Checks

- Run the parallel `shipper-core` tests and review any changed snapshots.
- If progress or narration changes, run the affected `shipper-cli` tests too.
