# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

## Purpose

This directory owns the advisory lock file used to prevent concurrent runs from mutating the same state directory.

## Key files

- `mod.rs` — lock path resolution, acquisition, release, and stale-lock handling.

## Invariants

- Lock acquisition semantics should stay conservative; do not accidentally weaken concurrent-run protection.
- Release remains best-effort, including drop-path cleanup.
- Changes to lock metadata or file naming affect resume/publish coordination across the tool.

## Checks

- Run the lock-related `shipper-core` tests.
- If lock behavior changes, also sanity-check the affected publish/resume flows.
