# shipper-encrypt

# shipper-encrypt

State file encryption using AES-256-GCM.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

State file encryption for shipper using AES-256-GCM

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-encrypt
cargo test -p shipper-encrypt
cargo test -p shipper-encrypt --all-features
cargo fmt -p shipper-encrypt
cargo clippy -p shipper-encrypt --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.