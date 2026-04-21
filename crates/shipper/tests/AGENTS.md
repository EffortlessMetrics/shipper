# AGENTS.md

## Purpose

This test tree covers the install-facing `shipper` facade as a product surface, not the internal engine implementation.

## Key files

- `facade_integration.rs` and `cross_crate_integration.rs` — facade and re-export expectations.
- `pipeline_integration.rs` and `integration_plan_engine.rs` — end-to-end behavior through the public crate surface.
- `schema_version_integration.rs` and `state_integration.rs` — compatibility and persisted-state expectations.

## Invariants

- These tests should exercise `shipper` as a thin facade over `shipper-cli` / `shipper-core`.
- If a failure points to engine behavior, fix the lower crate rather than adding facade-only workarounds here.
- Keep tests focused on the public `shipper` surface and product-level integration points.

## Checks

- Run `cargo test -p shipper`.
- If re-exports move, update the affected integration tests in the same change.
