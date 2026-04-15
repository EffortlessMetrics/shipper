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

| Crate | Purpose |
|-------|---------|
| `shipper-auth` | Token resolution (`CARGO_REGISTRY_TOKEN`, credentials.toml) |
| `shipper-cargo` | Workspace metadata via `cargo_metadata` |
| `shipper-cargo-failure` | Classify `cargo publish` stderr into typed failure categories |
| `shipper-chunking` | Split items into bounded-size chunks for parallel execution |
| `shipper-config` | Load, merge, and validate `.shipper.toml` configuration files; `runtime` submodule converts `ShipperConfig` + CLI overrides into `RuntimeOptions` |
| `shipper-duration` | Human-friendly duration parsing and serde codecs |
| `shipper-encrypt` | AES-256-GCM encryption for state files |
| `shipper-engine-parallel` | Wave-based parallel publish engine (dependency-level concurrency) |
| `shipper-environment` | Environment fingerprinting (OS, arch, CI, tool versions) |
| `shipper-events` | Append-only JSONL event log for audit trails |
| `shipper-execution-core` | Shared helpers for state updates, error classification, and backoff |
| `shipper-git` | Git operations (cleanliness check, branch/commit context) |
| `shipper-levels` | Group packages by dependency depth for parallel plans |
| `shipper-lock` | File-based advisory lock to prevent concurrent publishes |
| `shipper-output-sanitizer` | Redact tokens and secrets from captured cargo output |
| `shipper-plan` | Topological sort of publishable crates into a deterministic plan |
| `shipper-process` | Cross-platform command execution with timeout support |
| `shipper-progress` | TTY-aware progress bars for CLI publish workflows |
| `shipper-registry` | HTTP client for registry REST API (version check, owners) |
| `shipper-retry` | Configurable retry strategies (exponential, linear, constant) with jitter |
| `shipper-schema` | Schema-version parsing and compatibility validation |
| `shipper-sparse-index` | Cargo sparse-index path derivation and version lookup |
| `shipper-state` | Persistence of `state.json` (resumable execution state) |
| `shipper-storage` | Pluggable storage backends (filesystem, S3, GCS, Azure) |
| `shipper-store` | `StateStore` trait — high-level persistence abstraction |
| `shipper-types` | Core domain types (specs, plans, options, receipts, errors) |
| `shipper-webhook` | Webhook notifications for publish lifecycle events |

---

## Dependency Graph

Arrows read as "depends on". Only shipper-\* edges are shown.

```
shipper-cli
  ├── shipper  (facade)
  ├── shipper-duration
  └── shipper-progress

shipper  (facade — re-exports all microcrates)
  ├── shipper-types
  ├── shipper-config           (runtime conversion helpers live in `shipper_config::runtime`)
  ├── shipper-schema
  ├── shipper-retry
  ├── shipper-duration
  ├── shipper-levels
  ├── shipper-encrypt
  ├── shipper-webhook
  ├── shipper-cargo-failure
  ├── shipper-execution-core
  ├── shipper-output-sanitizer
  ├── shipper-sparse-index
  ├── shipper-auth            (optional, feature-gated)
  ├── shipper-cargo           (optional)
  ├── shipper-engine-parallel (optional)
  ├── shipper-environment     (optional)
  ├── shipper-events          (optional)
  ├── shipper-git             (optional)
  ├── shipper-lock            (optional)
  ├── shipper-plan            (optional)
  ├── shipper-process         (optional)
  ├── shipper-registry        (optional)
  ├── shipper-state           (optional)
  ├── shipper-storage         (optional)
  └── shipper-store           (optional)
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
  ├── shipper-types
  ├── shipper-encrypt
  ├── shipper-storage
  ├── shipper-webhook
  ├── shipper-retry
  └── shipper-schema

shipper-plan
  ├── shipper-cargo
  ├── shipper-state
  └── shipper-types

shipper-execution-core
  ├── shipper-cargo-failure
  ├── shipper-retry
  ├── shipper-state
  └── shipper-types

shipper-engine-parallel
  ├── shipper-chunking
  ├── shipper-execution-core
  ├── shipper-cargo
  ├── shipper-events
  ├── shipper-plan
  ├── shipper-registry
  ├── shipper-retry
  ├── shipper-state
  ├── shipper-types
  ├── shipper-sparse-index
  └── shipper-webhook

shipper-state
  ├── shipper-types
  ├── shipper-environment
  ├── shipper-encrypt
  └── shipper-schema

shipper-store
  ├── shipper-events
  ├── shipper-types
  ├── shipper-state
  └── shipper-schema

shipper-events ──► shipper-types
shipper-environment ──► shipper-types
shipper-registry ──► shipper-sparse-index
shipper-cargo ──► shipper-output-sanitizer

Leaf crates (zero shipper-* dependencies):
  shipper-auth, shipper-cargo-failure, shipper-chunking,
  shipper-duration, shipper-encrypt, shipper-git, shipper-levels,
  shipper-lock, shipper-output-sanitizer, shipper-process,
  shipper-progress, shipper-retry, shipper-schema,
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
parallel mode via `engine_parallel`):

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
  `shipper-git`.
- **Independent testability** — each crate has its own unit tests with no
  reliance on the full workspace.
- **Optional composition** — the facade uses Cargo features (`micro-auth`,
  `micro-git`, etc.) to make most microcrates optional, enabling slim builds
  for downstream consumers.

Fifteen of the 29 microcrates are **leaf crates** with zero internal
dependencies, enforcing loose coupling.

### Facade pattern

The `shipper` crate does not contain significant logic of its own. It
re-exports each microcrate as a module (e.g., `shipper::auth`, `shipper::plan`)
and provides `cfg`-gated module declarations that swap between local
implementations and microcrate re-exports based on the active feature set.
The CLI depends only on `shipper`, never on individual microcrates directly
(except `shipper-progress` and `shipper-duration`).

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

### Configuration layer

| Crate | Role |
|-------|------|
| `shipper-config` | Parse `.shipper.toml`, merge sections, validate constraints; `runtime` submodule converts `ShipperConfig` + `CliOverrides` → `RuntimeOptions` |
| `shipper-schema` | Parse and validate schema version identifiers in state files |

Configuration flows: CLI flags → `CliOverrides` → merged with `ShipperConfig`
from disk → produces `RuntimeOptions` consumed by the engine.

### Execution layer

| Crate | Role |
|-------|------|
| `shipper-engine-parallel` | Orchestrate wave-based parallel publish across dependency levels |
| `shipper-execution-core` | Shared helpers: state updates, failure classification, backoff delay |
| `shipper-cargo` | Run `cargo metadata` / `cargo publish` via subprocess |
| `shipper-cargo-failure` | Pattern-match `cargo publish` stderr into failure categories |
| `shipper-process` | Cross-platform process spawning with timeout support |
| `shipper-output-sanitizer` | Strip tokens and secrets from captured subprocess output |

The `engine` module inside the `shipper` facade implements the sequential
publish loop; `shipper-engine-parallel` extends this with dependency-level
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
| `shipper-state` | Read/write `state.json` with optional encryption |
| `shipper-store` | `StateStore` trait — high-level read/write/list for state + events |
| `shipper-storage` | `StorageBackend` trait and implementations (filesystem, S3, GCS, Azure) |
| `shipper-encrypt` | AES-256-GCM encrypt/decrypt primitives |
| `shipper-events` | Append-only JSONL event log writer |

### Infrastructure layer

| Crate | Role |
|-------|------|
| `shipper-auth` | Resolve registry tokens from env vars and credentials.toml |
| `shipper-registry` | HTTP client for registry API (version existence, owner queries) |
| `shipper-sparse-index` | Derive sparse-index paths and check index content for versions |
| `shipper-git` | Check working-tree cleanliness, capture branch/commit context |
| `shipper-lock` | File-based advisory lock with configurable staleness timeout |
| `shipper-environment` | Fingerprint the runtime environment (OS, arch, CI provider) |

### Types & utilities

| Crate | Role |
|-------|------|
| `shipper-types` | Core domain types: `ReleaseSpec`, `ReleasePlan`, `RuntimeOptions`, `Receipt`, errors |
| `shipper-duration` | Parse human-readable durations (`2s`, `5m`) and serde codecs |
| `shipper-retry` | Retry strategies (exponential / linear / constant) with jitter |
| `shipper-levels` | Dependency-level grouping data structure |
| `shipper-webhook` | Webhook payload types and HTTP delivery |
| `shipper-progress` | TTY-aware progress reporting for the CLI |

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

The facade crate uses Cargo features to gate microcrate dependencies:

```
micro-auth, micro-git, micro-events, micro-lock, micro-encrypt,
micro-environment, micro-storage, micro-cargo, micro-plan,
micro-registry, micro-process, micro-policy, micro-webhook,
micro-types, micro-config, micro-state, micro-store, micro-parallel
```

`micro-all` enables everything and is the default for `shipper-cli`. Downstream
library consumers can depend on `shipper` with only the features they need,
keeping compile times and binary size minimal.

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