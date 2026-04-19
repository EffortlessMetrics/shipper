# shipper-core

Lean engine library behind [Shipper](https://crates.io/crates/shipper).

`shipper-core` is the stable embedding surface for Shipper's workspace-publish engine. It has no CLI dependencies — no `clap`, no `indicatif`, no progress rendering — and is intended for Rust tools, CI frameworks, and tests that need to drive `cargo publish` with Shipper's safety guarantees but do not need the operator-facing CLI.

## Use this crate when

You want deterministic publish planning, preflight, publish/resume orchestration, and state/receipt/event handling **without** pulling the clap and terminal-UX graph.

If you just want to run Shipper from a terminal or CI, install [`shipper`](https://crates.io/crates/shipper) instead.

## What lives here

- **Plan** — deterministic dependency-ordered publish plan from `cargo_metadata`, with a stable `plan_id`.
- **Preflight** — git cleanliness, registry reachability, dry-run, version-not-taken, optional ownership checks.
- **Publish / resume** — per-crate `cargo publish` with retry/backoff, post-publish readiness verification, resumable state.
- **Reconciliation** — ambiguous-outcome handling against registry truth (`Published` / `NotPublished` / `StillUnknown`).
- **State / events / receipts** — append-only `events.jsonl` as truth, `state.json` as projection, `receipt.json` as summary.
- **Remediation planning** — yank, reverse-topological containment, fix-forward planning.
- **Rehearsal** — package + verify against an alternate registry before touching production.

## What does not live here

- CLI parsing (`clap`) — in `shipper-cli`.
- Progress rendering (`indicatif`) — in `shipper-cli`.
- Install-facing docs and binary — in `shipper`.

## Minimal shape

```rust
use shipper_core::plan::build_plan;

// See docs/ and the engine entry points for the full API.
// The load-bearing pieces are:
//   - shipper_core::plan   — build a ReleasePlan from a workspace
//   - shipper_core::engine — run preflight / publish / resume
//   - shipper_core::state  — read persisted execution state
//   - shipper_core::store  — StateStore trait + filesystem impl
//   - shipper_core::types  — domain types (specs, receipts, state, events)
//   - shipper_core::config — load and merge `.shipper.toml`
```

## Architecture

```text
shipper (install face)
  -> shipper-cli (CLI adapter: clap, subcommands, output, pub fn run())
       -> shipper-core (this crate — engine, no CLI deps)
```

## Related

- Install face: <https://crates.io/crates/shipper>
- CLI adapter: <https://crates.io/crates/shipper-cli>
- Project README: <https://github.com/EffortlessMetrics/shipper#readme>
- Architecture: <https://github.com/EffortlessMetrics/shipper/blob/main/docs/architecture.md>

## Stability

Pre-1.0. The public API will move; breaking changes are called out in [`CHANGELOG.md`](https://github.com/EffortlessMetrics/shipper/blob/main/CHANGELOG.md).

## License

MIT OR Apache-2.0.
