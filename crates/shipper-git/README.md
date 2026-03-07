# shipper-git

# shipper-git

Git operations for workspace cleanliness checks and tagging.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Git operations for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-git
cargo test -p shipper-git
cargo test -p shipper-git --all-features
cargo fmt -p shipper-git
cargo clippy -p shipper-git --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.