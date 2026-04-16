# Project Structure

> Snapshot of the repository layout. Source paths drift; check `git ls-files` if anything looks off.

## Workspace layout

```
shipper/
├── crates/                            # Workspace members
│   ├── shipper/                       # Core library — engine, plan, state, runtime
│   ├── shipper-cli/                   # Thin CLI binary (`shipper` command)
│   ├── shipper-config/                # .shipper.toml parsing/validation
│   ├── shipper-types/                 # Shared types (Plan, ExecutionState, Receipt, events)
│   ├── shipper-registry/              # Registry HTTP clients
│   ├── shipper-cargo-failure/         # Cargo error classification (ErrorClass patterns)
│   ├── shipper-duration/              # Human-readable duration parsing
│   ├── shipper-encrypt/               # State encryption (optional)
│   ├── shipper-output-sanitizer/      # ANSI strip + token redaction
│   ├── shipper-retry/                 # Retry/backoff strategies
│   ├── shipper-sparse-index/          # Sparse index protocol client
│   ├── shipper-webhook/               # Webhook delivery
│   ├── shipper-events/                # (workspace-internal)
│   ├── shipper-state/                 # (workspace-internal)
│   └── shipper-storage/               # (workspace-internal)
├── docs/                              # User & contributor documentation
│   ├── product.md                     # This area: orientation
│   ├── structure.md                   # This file
│   ├── tech.md                        # Tech stack
│   ├── INVARIANTS.md                  # Events-as-truth contract
│   ├── architecture.md
│   ├── configuration.md
│   ├── failure-modes.md
│   ├── preflight.md
│   ├── readiness.md
│   ├── release-runbook.md
│   └── testing.md
├── .github/workflows/                 # CI: ci.yml, release.yml, etc.
├── templates/                         # CI workflow snippets (github-actions, gitlab, ...)
├── features/                          # Cucumber/BDD scenarios
├── fuzz/                              # cargo-fuzz targets
├── MISSION.md                         # North star: mission, vision, beliefs
├── ROADMAP.md                         # Nine-competency thesis + sequencing
├── CLAUDE.md                          # AI: Claude Code context
├── GEMINI.md                          # AI: Gemini context
├── README.md                          # User entry point
├── CONTRIBUTING.md
├── SECURITY.md
└── CHANGELOG.md
```

12 of the 15 crates are published to crates.io as part of v0.3.0-rc.1. The remaining 3 (`shipper-events`, `shipper-state`, `shipper-storage`) are workspace-internal.

## `crates/shipper` module map

```
crates/shipper/src/
├── lib.rs                # Public API surface
├── engine/               # Plan/preflight/publish/resume execution
│   ├── mod.rs            # Top-level engine entry points (run_preflight, run_publish, run_resume)
│   └── parallel/         # Parallel publish + readiness verification
│       ├── publish.rs    # Per-crate publish loop with retry/backoff
│       └── readiness.rs  # Sparse-index + API visibility queries
├── runtime/              # Execution runtime + error classification
│   └── execution/        # ErrorClass classification, classify_cargo_failure
├── plan/                 # Workspace analysis + topo-sort + plan_id
├── state/                # Persistence layer
│   ├── execution_state/  # state.json (atomic writes)
│   ├── events/           # events.jsonl writer (append-only)
│   └── store/            # StateStore trait
├── ops/                  # Operations
│   ├── auth/             # Token resolution + OIDC detection (oidc.rs)
│   └── lock/             # File-based distributed locking
├── config.rs             # Internal config helpers
├── git.rs                # Git working-tree checks
├── encryption.rs         # State encryption (uses shipper-encrypt)
├── webhook.rs            # Webhook event emission (uses shipper-webhook)
├── types.rs              # Crate-internal type aliases
├── property_tests.rs     # proptest harnesses
└── stress_tests.rs       # Long-running validation
```

## Runtime files (`.shipper/`)

Per [INVARIANTS.md](INVARIANTS.md):

| File | Authority | Purpose |
|---|---|---|
| `events.jsonl` | **Truth** | Append-only event stream — every state transition |
| `state.json` | Projection | Serialized `ExecutionState` for fast resume |
| `receipt.json` | Summary | End-of-run audit summary |
| `lock` | — | Concurrent-publish guard |

## Tests

- **Unit tests** — alongside the code they cover (`#[cfg(test)] mod tests`)
- **Integration tests** — `crates/<crate>/tests/`
- **BDD scenarios** — `features/`
- **Fuzz targets** — `fuzz/fuzz_targets/`
- **Snapshots** — `insta`
- **Property tests** — `proptest`
- Tests touching env vars or filesystem use `#[serial]` from `serial_test`
- Registry interactions use `tiny_http` mock servers — never hit real registries
