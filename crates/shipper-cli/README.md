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

## Installing the adapter directly

The `shipper-cli` crate ships its own `shipper-cli` binary, a
three-line wrapper over `shipper_cli::run()` — the same entry point
the `shipper` install uses. Install the adapter directly when you
want the `shipper-cli` binary name (e.g., existing pipelines, or
you prefer the adapter crate explicitly):

```bash
# From crates.io — same code path as `cargo install shipper`
cargo install shipper-cli --locked

# From this repository
cargo install --path crates/shipper-cli --locked
```

Most users should prefer `cargo install shipper --locked`, which
installs a binary named `shipper` against the same adapter.

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
