# shipper-auth

# shipper-auth

Authentication and token resolution for Cargo registry authentication.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Authentication and token resolution for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-auth
cargo test -p shipper-auth
cargo test -p shipper-auth --all-features
cargo fmt -p shipper-auth
cargo clippy -p shipper-auth --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.