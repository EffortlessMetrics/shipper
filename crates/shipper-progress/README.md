# shipper-progress

# shipper-progress

CLI progress reporting utilities for publish operations.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

CLI progress reporting utilities for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-progress
cargo test -p shipper-progress
cargo test -p shipper-progress --all-features
cargo fmt -p shipper-progress
cargo clippy -p shipper-progress --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.