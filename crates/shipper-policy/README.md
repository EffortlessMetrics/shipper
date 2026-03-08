# shipper-policy

# shipper-policy

Publish policy evaluation logic for deciding which crates to publish.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Publish policy evaluation logic for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-policy
cargo test -p shipper-policy
cargo test -p shipper-policy --all-features
cargo fmt -p shipper-policy
cargo clippy -p shipper-policy --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.