# shipper-config

# shipper-config

Configuration file handling and `.shipper.toml` parsing.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Configuration file handling for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-config
cargo test -p shipper-config
cargo test -p shipper-config --all-features
cargo fmt -p shipper-config
cargo clippy -p shipper-config --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.