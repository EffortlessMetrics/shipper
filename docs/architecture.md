# Architecture

Shipper is a **publishing reliability layer** for Rust workspaces. It wraps
`cargo publish` with deterministic ordering, preflight checks, retry/backoff,
state persistence, and audit evidence — making multi-crate publishes safe to
start, safe to interrupt, and safe to re-run.

---

## Workspace Structure

The repository is a Cargo workspace with **31 crates**: one facade library, one
CLI binary, and 29 focused microcrates that each own a single responsibility.

### Facade & CLI

| Crate | Kind | Purpose |
|-------|------|---------|
| `shipper` | lib | Facade — re-exports every microcrate as a public module |
| `shipper-cli` | bin | Thin CLI that parses args via `clap` and delegates to the library |

### Microcrates

> **Note:** The project is consolidating many single-responsibility microcrates
> into module folders under `shipper`, `shipper-config`, and `shipper-cli`
> (the "decrating" effort). Entries below marked _Absorbed_ are no longer
> published as standalone crates. A post-absorption refresh will rewrite this
> section around the final module layout (`engine/`, `plan/`, `ops/`,
> `runtime/`, `store/`).

| Crate | Purpose |
|-------|---------|
| `shipper-cargo` | Workspace metadata via `cargo_metadata` |
| `shipper-cargo-failure` | Classify `cargo publish` stderr into typed failure categories |
| `shipper-chunking` | _Absorbed — now `shipper::plan::chunking` module (PR #56)_ |
| `shipper-config` | Load, merge, and validate `.shipper.toml` configuration files; `runtime` submodule converts `ShipperConfig` + CLI overrides into `RuntimeOptions` |
| `shipper-config-runtime` | _Absorbed — now `shipper-config::runtime` module (PR #58)_ |
| `shipper-duration` | Human-friendly duration parsing and serde codecs |
| `shipper-encrypt` | AES-256-GCM encryption for state files |
| `shipper-environment` | _Absorbed — now `shipper::runtime::environment` module (PR #65)_ |
| `shipper-events` | _Absorbed — now `shipper::state::events` module (PR #60)_ |
| `shipper-execution-core` | _Absorbed — now `shipper::runtime::execution` module (PR #69)_ |
| `shipper-git` | _Absorbed — now `shipper::ops::git` module (decrating Phase 2)_ |
| `shipper-levels` | _Absorbed — now `shipper::plan::levels` module (PR #56)_ |
| `shipper-lock` | _Absorbed — now `shipper::ops::lock` module (PR #52)_ |
| `shipper-output-sanitizer` | Redact tokens and secrets from captured cargo output |
| `shipper-plan` | _Absorbed — now `shipper::plan` module (PR #56)_ |
| `shipper-policy` | _Absorbed — now `shipper::runtime::policy` module (PR #54)_ |
| `shipper-process` | _Absorbed — now `shipper::ops::process` module (PR #55)_ |
| `shipper-progress` | _Absorbed — now `shipper-cli::output::progress` module (PR #67)_ |
| `shipper-registry` | HTTP client for registry REST API (version check, owners) |
| `shipper-retry` | Configurable retry strategies (exponential, linear, constant) with jitter |
| `shipper-schema` | _Folded into `shipper-types::schema` — schema-version parsing and validation now lives in `shipper-types` (Phase 6)_ |
| `shipper-sparse-index` | Cargo sparse-index path derivation and version lookup |
| `shipper-state` | _Absorbed — now `shipper::state::execution_state` module (PR #60)_ |
| `shipper-storage` | _SPLIT — config types to `shipper-types::storage`, backend to `shipper::ops::storage` (PR #68)_ |
| `shipper-store` | _Absorbed — now `shipper::state::store` module (PR #57)_ |
| `shipper-types` | Core domain types (specs, plans, options, receipts, errors) |
| `shipper-webhook` | Webhook notifications for publish lifecycle events |

---

## Dependency Graph

> **Note:** The graph below reflects the pre-decrating layout. Crates marked
> _Absorbed_ above no longer exist as standalone nodes — their edges are now
> intra-crate module edges inside `shipper`, `shipper-config`, or `shipper-cli`.
> A full redraw is deferred to the post-Phase-2 doc refresh.

Arrows read as "depends on". Only shipper-\* edges are shown.

```
shipper-cli
  ├── shipper  (facade)
  └── shipper-duration
  (progress UI lives inline at shipper-cli::output::progress)

shipper  (facade — re-exports all microcrates)
  ├── shipper-types            (includes schema module, formerly `shipper-schema`)
  ├── shipper-config           (runtime conversion helpers live in `shipper_config::runtime`)
  ├── shipper-retry
  ├── shipper-duration
  ├── shipper-levels
  ├── shipper-encrypt
  ├── shipper-webhook
  ├── shipper-cargo-failure
  ├── shipper-output-sanitizer
  ├── shipper-sparse-index
  ├── shipper-cargo           (optional)
  ├── shipper-registry
  └── shipper-sparse-index
```

### Microcrate internal edges

```
shipper-types
  ├── shipper-encrypt
  ├── shipper-webhook
  ├── shipper-retry
  ├── shipper-duration
  └── shipper-levels

shipper-config  (contains `runtime` submodule for config→RuntimeOptions conversion)
  ├── shipper-types            (for schema-version helpers + domain types)
  ├── shipper-encrypt
  ├── shipper-webhook
  └── shipper-retry

shipper-registry ──► shipper-sparse-index
shipper-cargo ──► shipper-output-sanitizer

Leaf crates (zero shipper-* dependencies):
  shipper-cargo-failure, shipper-chunking,
  shipper-duration, shipper-encrypt, shipper-levels,
  shipper-lock, shipper-output-sanitizer, shipper-process,
  shipper-retry,
  shipper-sparse-index, shipper-webhook
```

---

## Core Flow

Every publish operation follows the same pipeline:

```
 ┌────────┐    ┌───────────┐    ┌─────────┐    ┌────────┐    ┌─────────┐
 │  Plan  │───►│ Preflight │───►│ Publish │───►│ Verify │───►│ Receipt │
 └────────┘    └───────────┘    └─────────┘    └────────┘    └─────────┘
                                     │              ▲
                                     │  per crate   │
                                     └──────────────┘
```

### 1. Plan (`shipper plan`)

`plan::build_plan` reads the workspace manifest via `cargo_metadata`, filters
out `publish = false` crates and crates whose current version already exists on
the registry, then produces a **topologically sorted** `ReleasePlan`. The plan
is identified by a deterministic SHA-256 hash (`plan_id`) so that resume
operations can verify they match the original intent.

### 2. Preflight (`shipper preflight`)

`engine::run_preflight` runs all safety checks **without publishing**:

- **Git cleanliness** — working tree must be clean (unless `--allow-dirty`).
- **Dry-run compilation** — `cargo publish --dry-run` for each crate.
- **Version existence** — confirm the version is not already on the registry.
- **Ownership** — optionally verify the current user is an owner (best-effort).
- **Registry reachability** — verify the API endpoint responds.

The result is a `PreflightReport` with a `Finishability` verdict (all-good,
warnings-only, or blocking issues).

### 3. Publish (`shipper publish`)

`engine::run_publish` executes the plan crate-by-crate (or wave-by-wave in
parallel mode via `engine::parallel`):

- Runs `cargo publish -p <crate>` with `--no-verify` passthrough if requested.
- On failure, classifies the error (`cargo_failure`) and applies retry with
  configurable backoff (`retry`).
- Persists state to `.shipper/state.json` after every step so the run is
  resumable.
- Fires webhook events and appends to the JSONL event log.

### 4. Verify (readiness checks)

After each successful `cargo publish`, shipper polls the registry to confirm
the newly published version is visible:

- **API check** — HTTP GET to the registry REST API.
- **Index check** — sparse-index lookup for the version string.
- Configurable timeout, poll interval, and method (`api`, `index`, or `both`).

This prevents dependent crates from attempting to publish before their
dependencies are actually resolvable.

### 5. Receipt

On completion (or partial completion), shipper writes:

- `.shipper/state.json` — resumable execution state.
- `.shipper/receipt.json` — machine-readable audit receipt with per-crate
  evidence (attempt counts, durations, error classifications, timestamps).
- `.shipper/events.jsonl` — append-only structured event log.

### Resume (`shipper resume`)

`engine::run_resume` reloads persisted state, verifies the `plan_id` matches
(unless `--force-resume`), and continues from the first pending or failed
package. This makes shipper safe to use in CI where jobs may be cancelled and
restarted.

---

## Key Design Decisions

### SRP microcrate architecture

Each concern lives in its own crate with a minimal public API. This provides:

- **Fast incremental builds** — changing `shipper-retry` does not recompile
  unrelated microcrates like `shipper-webhook`.
- **Independent testability** — each crate has its own unit tests with no
  reliance on the full workspace.
- **Stable public surface** — the 13-crate target (achieved via the
  decrating effort) keeps semver promises and docs.rs pages small while
  preserving SRP at the module level inside the facade crate.

Several of the remaining microcrates are **leaf crates** with zero internal
dependencies, enforcing loose coupling.

### Facade pattern

The `shipper` crate does not contain significant logic of its own. It
re-exports each microcrate as a module (e.g., `shipper::auth`, `shipper::plan`)
and provides `cfg`-gated module declarations that swap between local
implementations and microcrate re-exports based on the active feature set.
The CLI depends only on `shipper`, never on individual microcrates directly
(except `shipper-duration`). Progress-bar UI lives inside `shipper-cli` itself
at `shipper-cli::output::progress`.

### State persistence for resumability

Execution state is serialised to `.shipper/state.json` after every crate
publish step. The state file records:

- The `plan_id` (SHA-256) to match against the current plan.
- Per-package status (`Pending`, `Publishing`, `Published`, `Failed`).
- Attempt counts and error history.
- Optional AES-256-GCM encryption (via `--encrypt`).

Resume verifies the plan hash before continuing, preventing accidental
cross-plan confusion. A pluggable `StorageBackend` trait allows state files to
be persisted to cloud storage (S3, GCS, Azure) for distributed CI.

### Registry verification with backoff

After each `cargo publish`, shipper does **not** assume the version is
immediately available. It actively polls the registry API and/or sparse index,
using configurable timeouts and intervals. This eliminates the most common
multi-crate publish failure: a dependent crate trying to resolve a dependency
that the registry has not yet indexed.

The retry layer (`shipper-retry`) provides exponential, linear, and constant
backoff strategies with configurable jitter, applied both to publish retries
and readiness polling.

---

## Module Responsibilities

> **Note:** Entries for _Absorbed_ crates (see the Microcrates table above)
> describe their pre-decrating roles. Those responsibilities now live in
> modules inside `shipper`, `shipper-config`, or `shipper-cli`. A full
> rewrite of this section is deferred to the post-Phase-2 doc refresh.

### Configuration layer

| Crate | Role |
|-------|------|
| `shipper-config` | Parse `.shipper.toml`, merge sections, validate constraints; `runtime` submodule converts `ShipperConfig` + `CliOverrides` → `RuntimeOptions` |
| `shipper-types::schema` | Parse and validate schema version identifiers in state files (formerly the standalone `shipper-schema` crate, folded in during Phase 6) |

Configuration flows: CLI flags → `CliOverrides` → merged with `ShipperConfig`
from disk → produces `RuntimeOptions` consumed by the engine.

### Execution layer

| Crate | Role |
|-------|------|
| `shipper-cargo` | Run `cargo metadata` / `cargo publish` via subprocess |
| `shipper-cargo-failure` | Pattern-match `cargo publish` stderr into failure categories |
| `shipper-process` | Cross-platform process spawning with timeout support |
| `shipper-output-sanitizer` | Strip tokens and secrets from captured subprocess output |

The `engine` module inside the `shipper` facade implements the sequential
publish loop; `engine::parallel` (absorbed from the former
`shipper-engine-parallel` microcrate) extends this with dependency-level
wave concurrency.

### Planning layer

| Crate | Role |
|-------|------|
| `shipper-plan` | Read workspace, filter publishable crates, topological sort |
| `shipper-levels` | Group packages by dependency depth for parallel wave planning |
| `shipper-chunking` | Subdivide waves into bounded-size chunks (`--max-concurrent`) |

### State & persistence layer

| Crate | Role |
|-------|------|
| `shipper::state::execution_state` | Read/write `state.json` with optional encryption (absorbed) |
| `shipper::state::store` | `StateStore` trait — high-level read/write/list for state + events (absorbed) |
| `shipper-encrypt` | AES-256-GCM encrypt/decrypt primitives |
| `shipper::state::events` | Append-only JSONL event log writer (absorbed) |

### Infrastructure layer

| Crate | Role |
|-------|------|
| `shipper-registry` | HTTP client for registry API (version existence, owner queries) |
| `shipper-sparse-index` | Derive sparse-index paths and check index content for versions |
| `shipper-lock` | File-based advisory lock with configurable staleness timeout |

### Types & utilities

| Crate | Role |
|-------|------|
| `shipper-types` | Core domain types: `ReleaseSpec`, `ReleasePlan`, `RuntimeOptions`, `Receipt`, errors |
| `shipper-duration` | Parse human-readable durations (`2s`, `5m`) and serde codecs |
| `shipper-retry` | Retry strategies (exponential / linear / constant) with jitter |
| `shipper-levels` | Dependency-level grouping data structure |
| `shipper-webhook` | Webhook payload types and HTTP delivery |

---

## CLI Commands

The `shipper` binary (crate `shipper-cli`) exposes these subcommands:

| Command | Description |
|---------|-------------|
| `plan` | Print the deterministic publish order |
| `preflight` | Run all safety checks without publishing |
| `publish` | Execute the plan (auto-resumes if matching state exists) |
| `resume` | Continue an interrupted publish run |
| `status` | Compare local versions against the registry |
| `doctor` | Print environment and auth diagnostics |
| `inspect-events` | View the structured event log |
| `inspect-receipt` | View the audit receipt with evidence |
| `ci <platform>` | Print CI configuration snippets (GitHub Actions, GitLab, CircleCI, Azure DevOps) |
| `clean` | Remove state files (optionally keep receipt) |
| `config init` | Generate a default `.shipper.toml` |
| `config validate` | Validate a configuration file |
| `completion <shell>` | Generate shell completions |

Global flags control registry, retry, readiness, policy, parallelism,
encryption, webhooks, and output format. CLI flags always override
`.shipper.toml` values.

---

## Compile-Time Feature Flags

The `shipper` facade and `shipper-cli` crates do not expose feature flags
for swapping implementations. Earlier RC builds used a `micro-*` feature
matrix to toggle between in-tree modules and standalone microcrates; both
the flags and the dual implementations were removed as part of the
decrating effort. The production code path is now the only code path.

Token resolution is provided in-crate by `crate::ops::auth` (re-exported
as `shipper::auth::*`); previously this was the standalone `shipper-auth`
microcrate.

---

## Testing Strategy

- **Unit tests** live alongside each microcrate. Leaf crates are tested in
  isolation with no mocking required.
- **Integration tests** in `shipper` and `shipper-cli` use `tiny_http` to mock
  registry responses, `tempfile` for filesystem isolation, and `serial_test` for
  tests that mutate environment variables.
- **Snapshot tests** via `insta` cover plan output, receipt format, and config
  serialisation.
- **Property-based tests** via `proptest` verify invariants (e.g., plan
  determinism, state round-trip).
- **Fuzz targets** under `fuzz/` exercise state loading and token resolution
  with `cargo-fuzz`.
- `#[forbid(unsafe_code)]` is set workspace-wide.