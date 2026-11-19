# shipper-core

Reusable publishing engine behind [Shipper](https://crates.io/crates/shipper).

`shipper-core` provides workspace planning, preflight, publish/resume
orchestration, registry reconciliation, and durable evidence without pulling
the Clap or terminal-UX dependency graph.

If you want the product CLI, install
[`shipper`](https://crates.io/crates/shipper). If a wrapper needs the exact
command adapter, use [`shipper-cli`](https://crates.io/crates/shipper-cli).

## Engine responsibilities

- **Plan** — build a deterministic dependency-ordered release plan with a
  stable `plan_id`.
- **Preflight** — check git, registry, dry-run, version, ownership, and policy
  readiness.
- **Publish and resume** — publish one crate at a time by default, or use
  bounded opt-in parallelism for independent crates at the same dependency
  level; verify visibility and persist progress after each package.
- **Reconciliation** — classify ambiguous outcomes as `Published`,
  `NotPublished`, or `StillUnknown`; never blind-retry `StillUnknown`.
- **Durable evidence** — keep `events.jsonl` authoritative, `state.json` as its
  resumable projection, `receipt.json` as a derived summary, and
  `reconciliation.json` as registry-truth evidence for ambiguous outcomes.
- **Bounded remediation** — plan containment and fix-forward actions without
  implying a live registry mutation.

CLI arguments, progress rendering, human output, and command-owned JSON
envelopes belong to `shipper-cli`.

## Embedding shape

```rust,no_run
use shipper_core::plan::build_plan;

// Build a plan, then use the engine entry points that match your workflow.
```

The main public modules cover planning, engine execution, types, configuration,
state, and storage. The `shipper` facade re-exports a curated subset for
consumers who prefer the product crate name.

## Support boundary

The public API is pre-1.0 and may change with documented migration guidance.
The doc-hidden `shipper_core::cli_bridge` module is an additive, unsupported
integration seam for `shipper-cli`; it is not a general embedding contract and
is not re-exported by the `shipper` facade.

Durable run liveness is deliberately fail-closed. The current affirmative
probe is Linux-only and requires exact captured process identity. Unsupported,
cross-host, missing, malformed, mismatched, or unreadable evidence remains
unknown. Lock acquisition and age-based lock stealing are not made
liveness-aware by the read-only observation surface.

See the current [support tiers](https://github.com/EffortlessMetrics/shipper/blob/main/docs/status/SUPPORT_TIERS.md)
and [0.5.0 migration guide](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/release/0.5.0-migration.md)
(which resolves only after release authority creates that tag).

## Architecture

```text
shipper (install facade and curated re-exports)
  -> shipper-cli (Clap adapter, rendering, exit behavior)
       -> shipper-core (this crate: engine, no CLI dependencies)
```

## License

MIT OR Apache-2.0.
