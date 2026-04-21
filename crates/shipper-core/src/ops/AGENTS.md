# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

## Purpose

This directory owns low-level I/O helpers: cargo, git, locks, process execution, auth, and storage primitives.

## Key files

- `auth/` — token resolution.
- `cargo/` — cargo metadata and publish subprocesses.
- `git/` — git cleanliness and context capture.
- `lock/` — advisory lock files.
- `process/` and `storage/` — subprocess and persistence helpers.

## Invariants

- Keep this layer free of engine/plan/state/runtime orchestration.
- Error text that is pinned by CLI snapshots should not drift accidentally.
- Prefer small wrappers with stable behavior over clever abstractions here.

## Checks

- Run the targeted `shipper-core` tests for the touched helper.
- If boundary rules change, make sure the architecture guard still passes.
