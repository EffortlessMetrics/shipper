# shipper-levels

# shipper-levels

Dependency level grouping for parallel publish plans.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Dependency level grouping for parallel shipper publish plans

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-levels
cargo test -p shipper-levels
cargo test -p shipper-levels --all-features
cargo fmt -p shipper-levels
cargo clippy -p shipper-levels --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.