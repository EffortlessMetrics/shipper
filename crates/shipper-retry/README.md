# shipper-retry

# shipper-retry

Retry strategies and exponential backoff policies for publish operations.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Retry strategies and backoff policies for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Type ownership

Retry *behavior* lives here: delay calculation, jitter, error-class selection,
and the retry loop. The configuration *values* — `RetryStrategyType`,
`RetryStrategyConfig`, and `PerErrorConfig` — are defined in
`shipper_types::retry` and re-exported from this crate, so
`shipper_retry::RetryStrategyConfig` remains the supported path. This keeps the
domain-contract crate free of a randomness dependency; see
[docs/architecture.md](../../docs/architecture.md) and issue #261.

## Development commands

```bash
cargo check -p shipper-retry
cargo test -p shipper-retry
cargo test -p shipper-retry --all-features
cargo fmt -p shipper-retry
cargo clippy -p shipper-retry --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.