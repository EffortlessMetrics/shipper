# shipper-webhook

# shipper-webhook

Webhook notifications for publish events and status updates.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Webhook notifications for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Source entry point

- Main entry: $entryPoint

## Development commands

`ash
cargo check -p shipper-webhook
cargo test -p shipper-webhook
cargo test -p shipper-webhook --all-features
cargo fmt -p shipper-webhook
cargo clippy -p shipper-webhook --all-targets --all-features -- -D warnings
`

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.