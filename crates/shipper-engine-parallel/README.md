# shipper-engine-parallel

# shipper-engine-parallel

Parallel publish execution engine for release plans.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Parallel publish execution for shipper release plans

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-engine-parallel
cargo test -p shipper-engine-parallel
cargo test -p shipper-engine-parallel --all-features
cargo fmt -p shipper-engine-parallel
cargo clippy -p shipper-engine-parallel --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.