# shipper-cli

CLI adapter for [Shipper](https://crates.io/crates/shipper).

**Most users should install `shipper`, not this crate:**

```bash
cargo install shipper --locked
```

## Use this crate when

You need the exact clap-based CLI surface programmatically — for example, a wrapper that invokes Shipper after extra preflight steps of your own:

```rust,no_run
fn main() -> anyhow::Result<()> {
    // ... custom preflight ...
    shipper_cli::run()
}
```

Or you want to install the adapter binary directly:

```bash
cargo install shipper-cli --locked   # same code path as `cargo install shipper`
```

For programmatic use **without** the `clap` graph, depend on [`shipper-core`](https://crates.io/crates/shipper-core) instead — that's the lean embedding surface.

## Architecture

```text
shipper (install face — `cargo install shipper`)
  -> shipper-cli (this crate — CLI adapter, pub fn run())
       -> shipper-core (engine, no CLI deps)
```

## Related

- Install face: <https://crates.io/crates/shipper>
- Engine library: <https://crates.io/crates/shipper-core>
- Project README: <https://github.com/EffortlessMetrics/shipper#readme>

## License

MIT OR Apache-2.0.
