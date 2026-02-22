# shipper-cli

`shipper-cli` provides the `shipper` command for reliable, resumable publishing
of Rust workspace crates.

This crate is the CLI frontend for the `shipper` library crate.

## Install

```bash
cargo install shipper-cli --locked
```

From this repository:

```bash
cargo install --path crates/shipper-cli --locked
```

## Quick start

```bash
shipper plan
shipper preflight
shipper publish
```

If a run is interrupted:

```bash
shipper resume
```

## Core commands

- `shipper plan` - print publish order and skipped packages.
- `shipper preflight` - run checks without publishing.
- `shipper publish` - execute publish and persist state.
- `shipper resume` - continue from previous state.
- `shipper status` - compare local versions to registry versions.
- `shipper doctor` - print environment and auth diagnostics.

## State and evidence files

By default, the CLI writes to `.shipper/`:

- `state.json` - resumable execution state.
- `receipt.json` - machine-readable publish receipt.
- `events.jsonl` - append-only event log.

Use `--state-dir <path>` to relocate these files.

## Configuration

Generate and validate project config:

```bash
shipper config init
shipper config validate
```

The config file is `.shipper.toml` in your workspace root unless overridden by `--config`.

## Authentication

Publishing is delegated to Cargo. API checks use Cargo-compatible token locations:

- `CARGO_REGISTRY_TOKEN`
- `CARGO_REGISTRIES_<NAME>_TOKEN`
- `$CARGO_HOME/credentials.toml`

## Related crates and docs

- Library crate: <https://crates.io/crates/shipper>
- Project README: <https://github.com/EffortlessMetrics/shipper#readme>
- Configuration reference: <https://github.com/EffortlessMetrics/shipper/blob/main/docs/configuration.md>
