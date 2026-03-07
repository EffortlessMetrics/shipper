# CLAUDE.md

This file provides agent-specific guidance for working in crate $name.

## Scope

- Crate: $name
- Path: crates/shipper-schema
- Workspace root: h:\Code\Rust\shipper
- Primary entry: $entryPoint

## Useful commands

`ash
cargo check -p shipper-schema
cargo test -p shipper-schema
cargo test -p shipper-schema --all-features
cargo fmt -p shipper-schema
cargo clippy -p shipper-schema --all-targets --all-features -- -D warnings
`

## Context

- Keep changes small and targeted to the crate’s existing abstractions.
- Preserve public API compatibility unless the request explicitly asks for breaking changes.
- When touching serialization or state formats, update tests and related snapshots in the same crate.
- Prefer using existing fixtures and helpers rather than introducing inline test data.

For full workspace guidance, see [$rootDocs\CLAUDE.md](H:\Code\Rust\shipper\CLAUDE.md).