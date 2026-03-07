# shipper-registry

# shipper-registry

Registry API client for version checks and crate visibility verification.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Registry API client for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-registry
cargo test -p shipper-registry
cargo test -p shipper-registry --all-features
cargo fmt -p shipper-registry
cargo clippy -p shipper-registry --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.