# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

## Purpose

This is the workspace entrypoint. Use it to orient yourself before diving into a crate or docs subtree.

## Key areas

- `MISSION.md`, `ROADMAP.md`, `docs/README.md` — product intent, priorities, and docs map.
- `crates/shipper-core` — publish engine and stateful release logic.
- `crates/shipper-cli` — clap types, command dispatch, help text, and terminal output.
- `crates/shipper` — install-facing facade and curated re-exports.
- `docs/` — operator, product, and architecture docs.

## Invariants

- Behavior changes belong in `shipper-core`.
- CLI surface and output changes belong in `shipper-cli`.
- `shipper` stays thin.
- `events.jsonl` is the source of truth; `state.json` is a projection; `receipt.json` is a summary.

## Checks

- Run the smallest relevant crate tests first.
- Before finishing broader changes, run `cargo fmt --all -- --check` and the relevant `cargo test -p ...` / `cargo clippy -p ...` commands.
