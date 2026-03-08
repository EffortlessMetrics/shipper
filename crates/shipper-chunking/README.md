# shipper-chunking

# shipper-chunking

Chunking helpers for bounded parallel publish execution.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Chunking helpers for bounded parallel publish execution

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-chunking
cargo test -p shipper-chunking
cargo test -p shipper-chunking --all-features
cargo fmt -p shipper-chunking
cargo clippy -p shipper-chunking --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.