# shipper

Installable release-execution facade for Rust workspaces.

A workspace release can fail after some crates publish and before the rest do. Cargo's own docs note that `cargo publish --workspace` is non-atomic and that a client timeout does not always mean the upload failed. Shipper gives you a plan, a preflight gate, durable state, and a recovery path for that failure mode.

## Install

```bash
cargo install shipper --locked
```

## Quick start

```bash
shipper plan        # preview the publish order
shipper preflight   # check readiness
shipper publish     # execute the plan
shipper resume      # continue after an interrupted run
```

`shipper --help` and `shipper <subcommand> --help` are the canonical command reference.

## What this crate is

`shipper` is the user-facing package — the one you install and the one that shows up on crates.io. It wraps:

- a small binary that forwards to the CLI adapter,
- a curated library re-export over the engine (`engine`, `plan`, `types`, `config`, `state`, `store`),
- product-facing documentation.

The actual work happens in two sibling crates:

- [`shipper-cli`](https://crates.io/crates/shipper-cli) — CLI adapter (clap parsing, subcommands, output, `pub fn run()`).
- [`shipper-core`](https://crates.io/crates/shipper-core) — engine library with no CLI dependencies.

## Use another crate when

- You want the lean embedding surface (no `clap`, no `indicatif`) → depend on [`shipper-core`](https://crates.io/crates/shipper-core).
- You need the exact clap-driven CLI surface programmatically (custom wrappers, pre-run hooks) → depend on [`shipper-cli`](https://crates.io/crates/shipper-cli) and call `shipper_cli::run()`.
- You want `shipper` as a library but without the `clap` graph → `shipper = { version = "...", default-features = false }`.

## Scope

Shipper handles publishing, retrying, resuming, rehearsing, yanking, and fix-forward planning. It does not decide version numbers, generate changelogs, tag releases, or create GitHub releases — pair it with your preferred versioning/release workflow.

## Documentation

- Project README: <https://github.com/EffortlessMetrics/shipper#readme>
- Full docs tree: <https://github.com/EffortlessMetrics/shipper/tree/main/docs>
- Configuration reference: <https://github.com/EffortlessMetrics/shipper/blob/main/docs/configuration.md>

## Stability

Pre-1.0. Breaking changes are called out in [`CHANGELOG.md`](https://github.com/EffortlessMetrics/shipper/blob/main/CHANGELOG.md).

## License

MIT OR Apache-2.0.
