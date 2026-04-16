# Architecture

Shipper is a **publishing reliability layer** for Rust workspaces. It wraps `cargo publish` with deterministic ordering, preflight checks, retry/backoff, ambiguity reconciliation, state persistence, and audit evidence — making multi-crate publishes safe to start, safe to interrupt, and safe to re-run.

> See [MISSION.md](../MISSION.md) for the *why*. This doc is the *how*.

---

## The architectural rule

**Crates are semver promises. Folders are ownership boundaries.**

Every published crate on crates.io is a public API surface that downstream users will pin and depend on. Every published crate is a versioning commitment we have to honor. So we do not split a crate every time we want a new ownership boundary — we use **modules** for that. We split a crate only when:

- it has a genuinely independent value as a library to other consumers, OR
- it carries a stable contract that we want to commit to versioning separately

Most of the time, an internal subsystem (auth, lock, planning, execution core, parallel engine, state store, events writer, etc.) is one of those things. It deserves an owner; it does not deserve a published crate.

This rule produces three consequences we hold to:

1. **The public crate count stays small.** Today: 12 crates. We don't grow that lightly.
2. **`pub(crate)` is the default.** Crate roots are curated facades; subsystems stay crate-private unless externally consumed.
3. **No deep lateral imports.** Subsystems talk through owner-facing module roots, not through each other's internal helpers. That preserves SRP without preserving every microcrate.

---

## Workspace layout

The workspace has **12 crates**, all published. There is no separate "workspace-internal" tier. When a concern needs an owner that doesn't deserve a public crate, it lives as a module inside the relevant owner crate.

### Primary surface

| Crate | Purpose |
|---|---|
| `shipper` | Core library — engine, plan, state, runtime, ops |
| `shipper-cli` | Thin CLI binary (`shipper` command) |
| `shipper-config` | `.shipper.toml` parsing, validation, and CLI-overlay |
| `shipper-types` | Shared domain types (Plan, ExecutionState, Receipt, events, schema) |
| `shipper-registry` | Registry HTTP client (REST API: version-existence, owners) |

### Published support crates

These are leaf utilities with genuine standalone value to other consumers:

| Crate | Purpose |
|---|---|
| `shipper-duration` | Human-friendly duration parsing + serde codecs |
| `shipper-retry` | Retry/backoff strategies (exponential, linear, constant) with jitter |
| `shipper-encrypt` | AES-256-GCM encryption for state files |
| `shipper-webhook` | Webhook notifications for publish lifecycle events |
| `shipper-sparse-index` | Cargo sparse-index path derivation and version lookup |
| `shipper-cargo-failure` | Classify `cargo publish` stderr into typed failure categories |
| `shipper-output-sanitizer` | Redact tokens and secrets from captured cargo output |

### Workspace internals — modules, not crates

Many subsystems that started as crates have been consolidated into modules under `shipper`, `shipper-config`, or `shipper-cli`. Each of these has a clear owner module root and a `pub(crate)` boundary; they are not on crates.io.

Examples (non-exhaustive — see [structure.md](structure.md) for the full module map):

- `shipper::ops::auth` — token resolution + OIDC detection
- `shipper::ops::lock` — file-based distributed locking
- `shipper::ops::process` — subprocess invocation + capture
- `shipper::plan` — workspace analysis, topo-sort, plan_id, levels, chunking
- `shipper::engine` — preflight + parallel publish + readiness verification
- `shipper::runtime::execution` — error classification, retry coordination
- `shipper::runtime::policy` — publish/verify/readiness policy resolution
- `shipper::runtime::environment` — environment fingerprinting
- `shipper::state::execution_state` — `state.json` writer (atomic)
- `shipper::state::events` — `events.jsonl` writer (append-only)
- `shipper::state::store` — `StateStore` trait
- `shipper-config::runtime` — config + CLI → `RuntimeOptions` conversion
- `shipper-cli::output::progress` — progress bars and TTY rendering

---

## Pipeline

The core flow is **plan → preflight → publish → (resume if interrupted)**.

```
                                  ┌─────────────────────────────┐
shipper plan ──────────────────► │ ReleasePlan (plan_id stable) │
                                  └─────────────────────────────┘
                                                │
                                                ▼
                                  ┌─────────────────────────────┐
shipper preflight ──────────────► │ Finishability assessment    │
                                  │ (Proven / NotProven / Failed)│
                                  └─────────────────────────────┘
                                                │
                                                ▼
                                  ┌─────────────────────────────┐
shipper publish ────────────────► │ ExecutionState              │
                                  │ events.jsonl (append)       │
                                  │ state.json (atomic update)  │
                                  │ receipt.json (end-of-run)   │
                                  └─────────────────────────────┘
                                                │
                                            ⚠ killed?
                                                │
                                                ▼
                                  ┌─────────────────────────────┐
shipper resume ─────────────────► │ Reload state, validate      │
                                  │ plan_id, skip published     │
                                  │ packages, continue          │
                                  └─────────────────────────────┘
```

### Plan
Reads workspace via `cargo_metadata`, filters publishable crates, topologically sorts by intra-workspace dependencies (Kahn's algorithm with `BTreeSet` for determinism), computes a SHA256-based `plan_id` over (workspace identity × dependency graph × versions). Same workspace state always produces the same `plan_id`.

### Preflight
Validates git cleanliness, registry reachability, performs a workspace dry-run, checks version-not-taken, optionally verifies ownership. Produces a `Finishability` (Proven / NotProven / Failed). For first-publish runs of brand-new crates, `NotProven` is the correct outcome — see [#100](https://github.com/EffortlessMetrics/shipper/issues/100).

### Publish
Executes the plan one crate at a time with retry/backoff. After each `cargo publish`, verifies registry visibility (sparse index and/or API) before advancing to dependent crates. Persists `ExecutionState` to disk after every step.

### Resume
Reloads `.shipper/state.json`, validates the `plan_id` matches the current workspace plan, skips already-published packages, continues from the first pending crate. Plan-ID mismatch refuses resume unless `--force-resume`.

---

## Key abstractions

| Trait / Type | Lives in | Purpose |
|---|---|---|
| `StateStore` | `shipper::state::store` | Persistence abstraction. Currently filesystem-backed; designed to host future cloud backends. |
| `Reporter` | `shipper::engine` | Pluggable output handler for publish/preflight progress. |
| `RegistryClient` | `shipper-registry` | Trait-based registry API access (mock-friendly). |
| `ErrorClass` | `shipper-types` | `Retryable` (HTTP 429, network) / `Permanent` (auth, version conflict) / `Ambiguous` (upload may have succeeded despite client error). Only `Retryable` triggers backoff today; `Ambiguous` reconciliation is the largest open gap ([#99](https://github.com/EffortlessMetrics/shipper/issues/99)). |
| `PublishPolicy` / `VerifyMode` / `ReadinessMethod` | `shipper-types` | Configuration enums controlling safety vs speed tradeoffs. |
| `Finishability` | `shipper-types` | Preflight assessment outcome. |

---

## State files

See [INVARIANTS.md](INVARIANTS.md) for the truth/projection/summary contract.

| File | Authority | Purpose |
|---|---|---|
| `events.jsonl` | **Truth** (append-only) | Every state transition |
| `state.json` | Projection | Serialized `ExecutionState` for resume |
| `receipt.json` | Summary | End-of-run audit summary |
| `lock` | — | Concurrent-publish guard |

---

## Dependency graph

Arrows read as "depends on"; only `shipper-*` edges are shown.

```
shipper-cli ──► shipper
shipper-cli ──► shipper-duration

shipper ──► shipper-types
shipper ──► shipper-config
shipper ──► shipper-registry
shipper ──► shipper-retry
shipper ──► shipper-duration
shipper ──► shipper-encrypt
shipper ──► shipper-webhook
shipper ──► shipper-cargo-failure
shipper ──► shipper-output-sanitizer
shipper ──► shipper-sparse-index

shipper-config ──► shipper-types
shipper-config ──► shipper-encrypt
shipper-config ──► shipper-webhook
shipper-config ──► shipper-retry

shipper-registry ──► shipper-sparse-index
shipper-registry ──► shipper-output-sanitizer

shipper-types ──► shipper-encrypt
shipper-types ──► shipper-webhook
shipper-types ──► shipper-retry
shipper-types ──► shipper-duration

Leaf crates (no shipper-* dependencies):
  shipper-cargo-failure, shipper-duration, shipper-encrypt,
  shipper-output-sanitizer, shipper-retry, shipper-sparse-index,
  shipper-webhook
```

---

## Conventions

- `unsafe_code = "forbid"` workspace-wide. No `unsafe` blocks anywhere.
- Edition 2024, MSRV 1.92, resolver v3.
- Tests touching env vars or filesystem use `#[serial]` from `serial_test` for isolation.
- Registry interactions in tests use `tiny_http` mock servers — never real registries.
- Snapshot tests use `insta`. Property-based tests use `proptest`.
- Token resolution follows Cargo conventions: `CARGO_REGISTRY_TOKEN` → `CARGO_REGISTRIES_<NAME>_TOKEN` → `$CARGO_HOME/credentials.toml`. Tokens are opaque strings, never logged.
- Atomic file writes everywhere (write-temp + fsync + rename + fsync-parent).
- `BTreeSet`/`BTreeMap` over `HashSet`/`HashMap` where iteration order is observable.

---

## See also

- [MISSION.md](../MISSION.md) — north star
- [ROADMAP.md](../ROADMAP.md) — five existential pillars + nine competencies
- [INVARIANTS.md](INVARIANTS.md) — events-as-truth contract
- [structure.md](structure.md) — module map
- [tech.md](tech.md) — tech stack
- [configuration.md](configuration.md) — `.shipper.toml` reference
- [preflight.md](preflight.md) — preflight checks
- [readiness.md](readiness.md) — readiness verification
- [failure-modes.md](failure-modes.md) — common failure scenarios
