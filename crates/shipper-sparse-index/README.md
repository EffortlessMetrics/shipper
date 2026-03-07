# shipper-sparse-index

# shipper-sparse-index

Cargo sparse-index path and version lookup helpers.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Cargo sparse-index path and version lookup helpers for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-sparse-index
cargo test -p shipper-sparse-index
cargo test -p shipper-sparse-index --all-features
cargo fmt -p shipper-sparse-index
cargo clippy -p shipper-sparse-index --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.