# shipper-cli

CLI adapter crate for Shipper. Owns `clap` parsing, subcommand dispatch,
help text, and progress rendering. Exposes `pub fn run() -> anyhow::Result<()>`
as the embedding entry point; the `shipper-cli` binary is a three-line
wrapper over `run`.

## Use the `shipper` crate to install

Most users should install the product via the `shipper` crate:

```bash
cargo install shipper --locked
```

That installs a binary named `shipper` that forwards to the same `run`
function in this crate.

## When to depend on `shipper-cli` directly

Reach for this crate when you need the exact CLI surface programmatically
— for example, a wrapper that invokes Shipper after extra preflight
steps of your own:

```rust,no_run
fn main() -> anyhow::Result<()> {
    // ... custom preflight ...
    shipper_cli::run()
}
```

For programmatic use **without** a `clap` dependency graph, depend on
[`shipper-core`](https://crates.io/crates/shipper-core) instead — that's
where the engine lives.

## Back-compat binary

A `shipper-cli` binary still ships so that existing pipelines with
`cargo install shipper-cli --locked` keep working. Prefer
`cargo install shipper --locked` on new setups.

```bash
# Backward-compatible — same code path
cargo install shipper-cli --locked

# From this repository
cargo install --path crates/shipper-cli --locked
```

## Core commands

- `shipper plan` — print publish order and skipped packages.
- `shipper preflight` — run checks without publishing.
- `shipper publish` — execute publish and persist state.
- `shipper resume` — continue from previous state.
- `shipper status` — compare local versions to registry versions.
- `shipper doctor` — print environment and auth diagnostics.
- `shipper rehearse` — package + verify + publish to a rehearsal registry.
- `shipper yank` / `shipper plan-yank` — receipt-driven containment.
- `shipper fix-forward` — plan a minimal repair of a partial release.

## Architecture

```
shipper (install face — `cargo install shipper`)
  -> shipper-cli (this crate — CLI adapter, pub fn run())
       -> shipper-core (engine — library only)
```

## Related crates and docs

- Install face: <https://crates.io/crates/shipper>
- Engine library: <https://crates.io/crates/shipper-core>
- Project README: <https://github.com/EffortlessMetrics/shipper#readme>
- Configuration reference: <https://github.com/EffortlessMetrics/shipper/blob/main/docs/configuration.md>
