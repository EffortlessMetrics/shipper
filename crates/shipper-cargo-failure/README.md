# shipper-cargo-failure

# shipper-cargo-failure

Cargo publish failure classification and error categorization.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Cargo publish failure classification for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-cargo-failure
cargo test -p shipper-cargo-failure
cargo test -p shipper-cargo-failure --all-features
cargo fmt -p shipper-cargo-failure
cargo clippy -p shipper-cargo-failure --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.