# shipper-process

Process execution and command invocation for cargo operations.

Provides utilities for running external processes with proper error handling,
timeouts, output capture, and environment variable support. Used by other
shipper crates to shell out to `cargo publish`, `cargo package`, and similar
commands with retry-friendly result types.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0
