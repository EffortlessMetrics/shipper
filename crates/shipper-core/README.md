# shipper-core

Core library behind the [`shipper`](https://crates.io/crates/shipper) CLI.

`shipper-core` is the engine, planning, state, registry, and remediation
layer. It has no CLI dependencies (no `clap`, no `indicatif`) and is
intended for programmatic use by CI frameworks, custom tools, and tests
that need to drive `cargo publish` with the same safety guarantees the
CLI gives operators.

## When to use which crate

- **Installing the CLI** — `cargo install shipper --locked`. Use the
  [`shipper`](https://crates.io/crates/shipper) crate.
- **Embedding the engine in your own Rust tool** — add
  `shipper-core` as a dependency.
- **Rewriting the CLI surface or writing a different frontend** — add
  [`shipper-cli`](https://crates.io/crates/shipper-cli), which re-exports
  `shipper-core` and owns the `clap` layer.

## Stability

Pre-1.0. The public API will move; breaking changes are called out in
[`CHANGELOG.md`](https://github.com/EffortlessMetrics/shipper/blob/main/CHANGELOG.md).
