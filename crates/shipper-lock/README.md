# shipper-lock

# shipper-lock

File-based locking to prevent concurrent publish runs.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

File-based locking mechanism for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-lock
cargo test -p shipper-lock
cargo test -p shipper-lock --all-features
cargo fmt -p shipper-lock
cargo clippy -p shipper-lock --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.