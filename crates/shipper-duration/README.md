# shipper-duration

# shipper-duration

Duration parsing and serde codecs for human-readable time values.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Duration parsing and serde codecs for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-duration
cargo test -p shipper-duration
cargo test -p shipper-duration --all-features
cargo fmt -p shipper-duration
cargo clippy -p shipper-duration --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.