# shipper-output-sanitizer

# shipper-output-sanitizer

Sanitize cargo command output before persistence and logging.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Sanitize cargo command output before persistence and logging

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Development commands

```bash
cargo check -p shipper-output-sanitizer
cargo test -p shipper-output-sanitizer
cargo test -p shipper-output-sanitizer --all-features
cargo fmt -p shipper-output-sanitizer
cargo clippy -p shipper-output-sanitizer --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.