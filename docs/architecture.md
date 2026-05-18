# Architecture

Shipper is a **publishing reliability layer** for Rust workspaces. It wraps
`cargo publish` with deterministic ordering, preflight checks, retry/backoff,
ambiguity reconciliation, state persistence, and audit evidence. The goal is to
make multi-crate publishes safe to start, safe to interrupt, and safe to re-run.

See [MISSION.md](../MISSION.md) for the why. This document is the crate and
module contract that keeps the implementation aligned with that product shape.

## The Architectural Rule

**Crates are semver promises. Folders are ownership boundaries.**

Every published crate on crates.io is a public API surface that downstream users
can pin and depend on. Every published crate is a versioning commitment we have
to honor. We do not split a crate every time we want a new ownership boundary;
we use modules for that. We split a crate only when:

- it has genuinely independent value as a library to other consumers, or
- it carries a stable contract that should be versioned separately.

This rule has three consequences:

1. **The public crate count stays small.** Today: 13 publishable crates. We do
   not grow that lightly.
2. **`pub(crate)` is the default.** Crate roots are curated facades; subsystems
   stay crate-private unless externally consumed.
3. **No deep lateral imports.** Subsystems talk through owner-facing module
   roots, not through each other's internal helpers.

## Product Crate Contract

The three product crates have separate jobs:

```text
shipper (install face and curated product-name library re-export)
  -> shipper-cli (CLI adapter: clap, subcommands, output, pub fn run())
       -> shipper-core (engine/library: plan, preflight, publish, resume)
```

This direction is load-bearing:

- `shipper` is the supported install facade. Its binary forwards to
  `shipper_cli::run()`, and its library re-exports a curated subset of
  `shipper-core` for callers that prefer the product name. While Shipper is
  prerelease-only on crates.io, registry installs need an explicit `--version`;
  local checkout install smoke uses `cargo install --path crates/shipper --locked`.
- `shipper-cli` owns command parsing, help text, progress rendering, snapshots,
  and human/JSON output. It maps user intent to `shipper-core`.
- `shipper-core` owns release behavior: plan, preflight, publish, resume,
  reconciliation, registry profiles, state, events, receipts, and remediation
  primitives. It must not depend on `clap`, `indicatif`, `shipper-cli`, or the
  `shipper` facade.

If the crate graph violates this direction, the graph is wrong. Do not paper
over it in docs.

## Workspace Layout

The workspace has **14 packages** in `cargo metadata`: 13 publishable crates and
one private `xtask` package. There is no separate publishable
"workspace-internal" tier. When a concern needs an owner that does not deserve a
public crate, it lives as a module inside the relevant owner crate.

### Product Surface

| Crate | Purpose |
|---|---|
| `shipper` | Install facade and product-name library re-export. Carries the `shipper` binary. |
| `shipper-cli` | Real CLI adapter: `clap`, subcommands, output, progress, and `pub fn run()`. |
| `shipper-core` | Engine/library surface: plan, preflight, publish, resume, reconcile, state/events/receipts. |

### Published Support Crates

These crates have standalone value or a stable contract worth versioning:

| Crate | Purpose |
|---|---|
| `shipper-config` | `.shipper.toml` parsing, validation, and CLI overlay. |
| `shipper-types` | Shared domain types: plans, execution state, receipts, events, and schemas. |
| `shipper-registry` | Registry HTTP client for version-existence and owner checks. |
| `shipper-duration` | Human-friendly duration parsing and serde codecs. |
| `shipper-retry` | Retry/backoff strategies with jitter. |
| `shipper-encrypt` | AES-256-GCM encryption for state files. |
| `shipper-webhook` | Webhook notifications for publish lifecycle events. |
| `shipper-sparse-index` | Cargo sparse-index path derivation and version lookup. |
| `shipper-cargo-failure` | Cargo publish stderr classification. |
| `shipper-output-sanitizer` | Token and secret redaction from captured cargo output. |

### Private Tooling Package

| Package | Purpose |
|---|---|
| `xtask` | Repository policy, release-evidence, and maintenance tooling. It is private and must stay unpublished. |

### Module Ownership

Many subsystems that started as crates have been consolidated into modules under
`shipper-core`, `shipper-config`, or `shipper-cli`. Each has a clear owner
module root and a `pub(crate)` boundary; they are not separately published.

Examples:

- `shipper_core::ops::auth` - token resolution and OIDC detection
- `shipper_core::ops::lock` - file-based distributed locking
- `shipper_core::ops::process` - subprocess invocation and capture
- `shipper_core::plan` - workspace analysis, topo-sort, plan ID, levels, chunking
- `shipper_core::engine` - preflight, publish, resume, reconcile, readiness verification
- `shipper_core::runtime::execution` - error classification and retry coordination
- `shipper_core::runtime::policy` - publish/verify/readiness policy resolution
- `shipper_core::runtime::environment` - environment fingerprinting
- `shipper_core::state::execution_state` - atomic `state.json` writer
- `shipper_core::state::events` - append-only `events.jsonl` writer
- `shipper_core::state::store` - `StateStore` trait
- `shipper-config::runtime` - config plus CLI overlay to `RuntimeOptions`
- `shipper-cli::output::progress` - progress bars and TTY rendering

## Package-Surface Contract

`cargo xtask package-surface` is the cheap architecture drift check. It writes
`target/policy/package-surface-report.{json,md}` and fails if the
facade/adapter/core dependency direction or private-tooling boundary drifts.
On the current graph it should report:

```text
workspace packages: 14
publishable packages: 13
private packages: 1
private package: xtask
```

For this architecture contract, the important fields are package counts,
publishability, targets, workspace dependencies, surface hashes, and the
`architecture_contract` section. That section proves:

- `shipper` depends on `shipper-cli` and `shipper-core`;
- `shipper-cli` depends on `shipper-core`;
- `shipper-core` has no normal, dev, or build dependency on `shipper`,
  `shipper-cli`, `clap`, or `indicatif`;
- `xtask` is the only private workspace package.

Any change that makes `shipper-core` depend on `shipper-cli` or `shipper`, turns
`xtask` publishable, removes the `shipper` install facade, or adds a new
publishable crate must update this document and `docs/status/SUPPORT_TIERS.md`
in the same PR.

## Release Pipeline

The core flow is:

```text
shipper plan
  -> shipper preflight
    -> shipper publish
      -> shipper resume, if interrupted
```

### Plan

Reads the workspace via `cargo_metadata`, filters publishable crates,
topologically sorts by intra-workspace dependencies, and computes a stable
`plan_id` over workspace identity, dependency graph, and versions.

### Preflight

Validates git cleanliness, registry reachability, local packageability,
version-not-taken checks, ownership checks where possible, and registry pacing
estimates. It produces a `Finishability` assessment:

- `Proven`
- `NotProven`
- `Failed`

For first-publish runs of brand-new crates, `NotProven` can be the correct
outcome because production registry visibility cannot be proven before publish.

### Publish

Executes the plan with registry-aware retry/backoff. It uses registry profiles,
honors retry floors when available, verifies readiness before advancing to
dependents, and persists state/events continuously.

Ambiguous cargo outcomes are reconciled against registry truth before retry.
The state machine is:

- `Published` - mark complete and continue without retry
- `NotPublished` - retry according to policy
- `StillUnknown` - stop and require operator action

### Resume

Reloads `.shipper/state.json`, validates the `plan_id` against the current
workspace plan, skips already-published packages, and reconciles ambiguous
states before continuing.

## Key Abstractions

| Trait / Type | Lives in | Purpose |
|---|---|---|
| `StateStore` | `shipper_core::state::store` | Persistence abstraction. Filesystem-backed today; designed for future backends. |
| `Reporter` | `shipper_core::engine` | Pluggable output handler for publish/preflight progress. |
| `RegistryClient` | `shipper-registry` | Trait-based registry API access for mock-friendly registry checks. |
| `ErrorClass` | `shipper-types` | `Retryable`, `Permanent`, and `Ambiguous` cargo failure classification. |
| `PublishReconciliation` | `shipper-types` | Registry-truth reconciliation evidence for ambiguous publish outcomes. |
| `PublishPolicy` / `VerifyMode` / `ReadinessMethod` | `shipper-types` | Configuration enums controlling safety/speed tradeoffs. |
| `Finishability` | `shipper-types` | Preflight proof outcome. |

## State Files

See [INVARIANTS.md](INVARIANTS.md) for the truth/projection/summary contract.

| File | Authority | Purpose |
|---|---|---|
| `events.jsonl` | Truth, append-only | Every state transition. |
| `state.json` | Projection | Serialized `ExecutionState` for resume. |
| `receipt.json` | Summary | End-of-run audit summary. |
| `lock` | Guard | Concurrent-publish protection. |

## Dependency Graph

Arrows read as "depends on"; only `shipper-*` edges are shown.

```text
shipper -> shipper-cli (optional, default feature)
shipper -> shipper-core

shipper-cli -> shipper-core
shipper-cli -> shipper-types
shipper-cli -> shipper-config
shipper-cli -> shipper-duration
shipper-cli -> shipper-retry

shipper-core -> shipper-types
shipper-core -> shipper-config
shipper-core -> shipper-registry
shipper-core -> shipper-retry
shipper-core -> shipper-duration
shipper-core -> shipper-encrypt
shipper-core -> shipper-webhook
shipper-core -> shipper-cargo-failure
shipper-core -> shipper-output-sanitizer
shipper-core -> shipper-sparse-index

shipper-config -> shipper-types
shipper-config -> shipper-encrypt
shipper-config -> shipper-webhook
shipper-config -> shipper-retry

shipper-registry -> shipper-sparse-index
shipper-registry -> shipper-output-sanitizer

shipper-types -> shipper-encrypt
shipper-types -> shipper-webhook
shipper-types -> shipper-retry
shipper-types -> shipper-duration

Leaf support crates:
  shipper-cargo-failure, shipper-duration, shipper-encrypt,
  shipper-output-sanitizer, shipper-retry, shipper-sparse-index,
  shipper-webhook
```

## Conventions

- `unsafe_code = "forbid"` workspace-wide.
- Edition 2024, MSRV 1.95, resolver v3.
- Tests touching env vars or filesystem use `#[serial]` from `serial_test`.
- Registry interactions in tests use `tiny_http` mock servers, never real
  registries.
- Snapshot tests use `insta`. Property-based tests use `proptest`.
- Tokens are opaque strings and must never be logged.
- Atomic file writes use write-temp, fsync, rename, and parent fsync.
- Prefer `BTreeSet`/`BTreeMap` where iteration order is observable.

## See Also

- [MISSION.md](../MISSION.md) - north star
- [ROADMAP.md](../ROADMAP.md) - five pillars and nine competencies
- [INVARIANTS.md](INVARIANTS.md) - events-as-truth contract
- [structure.md](structure.md) - module map
- [tech.md](tech.md) - tech stack
- [configuration.md](configuration.md) - `.shipper.toml` reference
- [preflight.md](preflight.md) - preflight checks
- [readiness.md](readiness.md) - readiness verification
- [failure-modes.md](failure-modes.md) - common failure scenarios
