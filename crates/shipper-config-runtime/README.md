# shipper-config-runtime

# shipper-config-runtime

Config-to-types conversion helpers for runtime configuration.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Config to types conversion helpers for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-config-runtime
cargo test -p shipper-config-runtime
cargo test -p shipper-config-runtime --all-features
cargo fmt -p shipper-config-runtime
cargo clippy -p shipper-config-runtime --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.