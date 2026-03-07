# shipper-store

# shipper-store

State store abstraction layer for shipper persistence.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

State store abstraction for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-store
cargo test -p shipper-store
cargo test -p shipper-store --all-features
cargo fmt -p shipper-store
cargo clippy -p shipper-store --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.