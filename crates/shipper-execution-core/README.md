# shipper-execution-core

# shipper-execution-core

Core execution helpers shared across publish engines.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Core execution helpers shared across shipper publish engines

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-execution-core
cargo test -p shipper-execution-core
cargo test -p shipper-execution-core --all-features
cargo fmt -p shipper-execution-core
cargo clippy -p shipper-execution-core --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.