# shipper-cli

Command adapter for [Shipper](https://crates.io/crates/shipper).

Most users should install the `shipper` facade, not this crate. The unversioned
command resolves the latest version exposed by the public registry when it
runs:

```bash
cargo install shipper --locked
```

The retained public-install evidence available when this source snapshot was
prepared covers facade version 0.4.0. Pin that baseline when reproducibility
matters:

```bash
cargo install shipper --version 0.4.0 --locked
```

This snapshot was prepared for the 0.5.0 candidate before its public result
existed. Package source or README text does not by itself prove registry
publication; check the live registry and release evidence.

## Use this crate when

Use `shipper-cli` only when a Rust wrapper needs Shipper's exact Clap command
surface and exit behavior programmatically:

```rust,no_run
fn main() -> anyhow::Result<std::process::ExitCode> {
    // Add wrapper-specific setup before entering the Shipper command adapter.
    shipper_cli::run()
}
```

For engine integration without Clap or terminal rendering, depend on
[`shipper-core`](https://crates.io/crates/shipper-core). Operators should run
the `shipper` facade.

## Ownership

`shipper-cli` owns:

- Clap arguments, subcommands, conflicts, and help text;
- command dispatch and process exit behavior;
- human rendering and versioned JSON command envelopes;
- the public `shipper_cli::run()` adapter entry point.

Engine behavior, durable event folding, state, receipts, registry policy, and
publish/resume orchestration remain in `shipper-core`.

The 0.5.0 candidate's `status --durable` mode reads core-owned observation
results, bypasses registry access, and emits `shipper.status.durable.v1` in JSON
mode. It does not turn raw `inspect-events` JSONL or direct receipt JSON into a
new command-owned envelope.

## Architecture

```text
shipper (install facade)
  -> shipper-cli (this crate: Clap adapter, rendering, exit behavior)
       -> shipper-core (engine, no CLI dependencies)
```

## Related

- [Install facade](https://crates.io/crates/shipper)
- [Engine library](https://crates.io/crates/shipper-core)
- [Project README](https://github.com/EffortlessMetrics/shipper#readme)
- [CLI reference](https://github.com/EffortlessMetrics/shipper/blob/main/docs/reference/cli.md)
- [Support tiers](https://github.com/EffortlessMetrics/shipper/blob/main/docs/status/SUPPORT_TIERS.md)

## License

MIT OR Apache-2.0.
