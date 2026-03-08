# shipper-cargo

# shipper-cargo

Cargo workspace metadata extraction and analysis.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Cargo workspace metadata for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-cargo
cargo test -p shipper-cargo
cargo test -p shipper-cargo --all-features
cargo fmt -p shipper-cargo
cargo clippy -p shipper-cargo --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.