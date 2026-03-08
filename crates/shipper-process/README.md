# shipper-process

# shipper-process

Process execution and command invocation for cargo operations.

Provides utilities for running external processes with proper error handling,
timeouts, output capture, and environment variable support. Used by other
shipper crates to shell out to `cargo publish`, `cargo package`, and similar
commands with retry-friendly result types.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Process execution for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-process
cargo test -p shipper-process
cargo test -p shipper-process --all-features
cargo fmt -p shipper-process
cargo clippy -p shipper-process --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.