# shipper-types

# shipper-types

Core types for plans, states, receipts, and publish operations.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Core types for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Dependency direction

This is the domain-contract hub: behavior crates depend on it, not the reverse.
It owns pure values, plans, events, state, receipts, and serialized contracts —
never network clients, cryptographic execution, retry sleeps, environment
access, or filesystem I/O. `cargo xtask package-surface` enforces the edges that
have already been inverted; see [docs/architecture.md](../../docs/architecture.md)
and issue #261.

## Development commands

```bash
cargo check -p shipper-types
cargo test -p shipper-types
cargo test -p shipper-types --all-features
cargo fmt -p shipper-types
cargo clippy -p shipper-types --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.