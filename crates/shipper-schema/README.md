# shipper-schema

# shipper-schema

Schema version parsing and validation for state files.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Schema version parsing and validation for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-schema
cargo test -p shipper-schema
cargo test -p shipper-schema --all-features
cargo fmt -p shipper-schema
cargo clippy -p shipper-schema --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.