# shipper

Reliable, resumable `cargo publish` for Rust workspaces.

```bash
cargo install shipper --locked
shipper plan
shipper preflight
shipper publish
```

The `shipper` crate is the install façade: it owns the CLI binary that
users invoke, and re-exports the public library API from
[`shipper-core`](https://docs.rs/shipper-core) so `use shipper::…`
keeps working for library consumers.

- **CLI users**: `cargo install shipper --locked`. See the main
  [README](https://github.com/EffortlessMetrics/shipper) for tutorials
  and the runbook.
- **Library users**: `shipper = "0.3"` as a dependency; everything
  that used to live in the standalone `shipper` crate is still here
  at the same paths (`shipper::engine`, `shipper::plan`,
  `shipper::types`, ...).

Related crates:

- [`shipper-core`](https://crates.io/crates/shipper-core) — the
  publishable library.
- [`shipper-cli`](https://crates.io/crates/shipper-cli) — the CLI
  argument parsing + command dispatch.

## License

Dual-licensed under MIT OR Apache-2.0.
