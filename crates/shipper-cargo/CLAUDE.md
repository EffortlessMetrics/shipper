# CLAUDE.md

This file provides agent-specific guidance for working in crate shipper-cargo.

## Scope

- Crate: shipper-cargo
- Path: crates/shipper-cargo
- Workspace root: h:\Code\Rust\shipper
- Primary entry: src/lib.rs

## Useful commands

```bash
cargo check -p shipper-cargo
cargo test -p shipper-cargo
cargo test -p shipper-cargo --all-features
cargo fmt -p shipper-cargo
cargo clippy -p shipper-cargo --all-targets --all-features -- -D warnings
```

## Context

- Keep changes small and targeted to the crate’s existing abstractions.
- Preserve public API compatibility unless the request explicitly asks for breaking changes.
- When touching serialization or state formats, update tests and related snapshots in the same crate.
- Prefer using existing fixtures and helpers rather than introducing inline test data.

For full workspace guidance, see [../../CLAUDE.md](H:\Code\Rust\shipper\CLAUDE.md).