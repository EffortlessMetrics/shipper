# shipper-plan

# shipper-plan

Workspace planning and dependency-aware publish ordering.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Workspace planning and dependency ordering for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-plan
cargo test -p shipper-plan
cargo test -p shipper-plan --all-features
cargo fmt -p shipper-plan
cargo clippy -p shipper-plan --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.