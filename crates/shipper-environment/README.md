# shipper-environment

# shipper-environment

Environment fingerprinting for reproducible publish runs.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Environment fingerprinting for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-environment
cargo test -p shipper-environment
cargo test -p shipper-environment --all-features
cargo fmt -p shipper-environment
cargo clippy -p shipper-environment --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.