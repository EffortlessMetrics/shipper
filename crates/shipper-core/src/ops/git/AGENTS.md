# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

## Purpose

This directory owns git cleanliness checks and git context capture for receipts and preflight.

## Key files

- `mod.rs` — public entrypoints used by the rest of `shipper-core`.
- `cleanliness.rs` — clean/dirty checks and error phrasing.
- `context.rs` — branch, commit, tag, and changed-file capture.
- `bin_override.rs` — `SHIPPER_GIT_BIN` override handling for tests and custom environments.

## Invariants

- Keep CLI-visible clean/dirty error wording stable unless the snapshot updates are intentional.
- `SHIPPER_GIT_BIN` must keep working for tests and sandboxed setups.
- Unknown or partial git state should stay safe-by-default.

## Checks

- Run the git-related `shipper-core` tests.
- If messages change, run the affected `shipper-cli` preflight/help snapshots too.
