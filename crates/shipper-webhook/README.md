# shipper-webhook

# shipper-webhook

Webhook notifications for publish events and status updates.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## License

MIT OR Apache-2.0


## Purpose

Webhook notifications for shipper

This crate is part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace.

## Type ownership

Delivery behavior lives here: HTTP transport, HMAC signing, and payload
rendering. The configuration *values* — `WebhookConfig` and `WebhookType` — are
defined in `shipper_types::webhook` and re-exported from this crate, so
`shipper_webhook::WebhookConfig` remains the supported path. This keeps the
domain-contract crate free of an HTTP/TLS dependency; see
[docs/architecture.md](../../docs/architecture.md) and issue #261.

## Development commands

```bash
cargo check -p shipper-webhook
cargo test -p shipper-webhook
cargo test -p shipper-webhook --all-features
cargo fmt -p shipper-webhook
cargo clippy -p shipper-webhook --all-targets --all-features -- -D warnings
```

## Contributing

When changing behavior, prefer extending existing tests in the crate module (	ests/, src/) and keep snapshots or properties in place where they already exist.