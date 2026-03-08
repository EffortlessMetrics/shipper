# shipper-storage

# shipper-storage

Storage backends for state and receipt persistence.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Storage backends for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-storage
cargo test -p shipper-storage
cargo test -p shipper-storage --all-features
cargo fmt -p shipper-storage
cargo clippy -p shipper-storage --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.