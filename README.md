# shipper

`shipper` is a **publishing reliability layer** for Rust workspaces.

Cargo already knows how to package and upload crates. What tends to break in real life is the workflow around it:

- publishing is **irreversible** (versions can’t be overwritten)
- multi-crate publishing can be **non-atomic** (partial publishes)
- CI runs get cancelled or time out
- registries apply **backpressure** (HTTP 429)
- some failures are **ambiguous** (upload may have succeeded even when the client errors)

Shipper is intentionally narrow: it focuses on making publishing **safe to start** and **safe to re-run**.

## What shipper does

- Builds a deterministic **publish plan** for workspace crates (dependency-first ordering).
- Runs **preflight checks** (git cleanliness, publishability, registry reachability, version existence).
- Optionally verifies **crate ownership/permissions** up front (when a token is available).
- Publishes **one crate at a time** using `cargo publish -p <crate>`.
- Applies retry/backoff for retryable failures.
- Verifies publish completion via the registry API before declaring success.
- Persists progress to disk so you can **resume** after interruption.

## What shipper does not do (yet)

- It does not bump versions, generate changelogs, create git tags, or create GitHub releases.
  Use `cargo-release`, `release-plz`, or your own workflow to decide *what* version to publish.
  Shipper focuses on *getting those versions published reliably*.

## Build / install

From this repository:

```bash
cargo build --release
cargo install --path crates/shipper-cli --locked
```

## Quick start (local)

```bash
shipper plan
shipper preflight
shipper publish
```

If the publish is interrupted (CI cancellation, network issues):

```bash
shipper resume
```

## Authentication

Publishing itself is performed by Cargo.

Shipper’s registry API checks (version existence, optional owners preflight) resolve a token using the same places people already use for Cargo:

- `CARGO_REGISTRY_TOKEN` (crates.io)
- `CARGO_REGISTRIES_<NAME>_TOKEN` (other registries; `<NAME>` uppercased, `-` replaced with `_`)
- `$CARGO_HOME/credentials.toml` (created by `cargo login`)

The token is treated as an **opaque string** and sent as the value of the `Authorization` header, matching Cargo’s registry web API model.

## State + receipts

By default Shipper writes:

- `.shipper/state.json` — resumable execution state
- `.shipper/receipt.json` — machine-readable receipt for CI/auditing

Use `--state-dir <path>` to redirect these elsewhere (for example, a CI artifacts directory).

## Commands

- `shipper plan` — print the publish order and what will be skipped
- `shipper preflight` — run checks without publishing
- `shipper publish` — execute the plan (writes state + receipt)
- `shipper resume` — continue from the last state
- `shipper status` — compare local versions to the registry
- `shipper doctor` — environment and auth diagnostics

## CI templates

See `templates/` for example workflows.

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license

at your option.
