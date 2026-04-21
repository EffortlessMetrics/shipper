# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

## Purpose

This directory orchestrates the release pipeline: plan -> preflight -> publish -> resume.

## Key files

- `mod.rs` — main entrypoints and the `Reporter` trait.
- `parallel/` — dependency-level parallel publish execution.

## Invariants

- Orchestration belongs here; clap parsing and terminal presentation do not.
- State persistence, retries, and publish/readiness flow should stay coherent with the serial pipeline.
- Changes here often affect CLI snapshots and resume/publish behavior across crates.

## Checks

- Run the targeted `shipper-core` tests that cover the changed engine path.
- If user-visible output or workflow behavior changes, run the relevant `shipper-cli` tests too.
