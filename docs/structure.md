# Project Structure

> Snapshot of the repository layout. Source paths drift; check `git ls-files` if anything looks off.

## Workspace layout

```
shipper/
├── crates/                            # Workspace members
│   ├── shipper/                       # Install façade (binary + curated lib re-export of shipper-core)
│   ├── shipper-cli/                   # CLI adapter: clap parsing, subcommands, output (pub fn run())
│   ├── shipper-core/                  # Engine: plan, preflight, publish, resume, state, ops (no CLI deps)
│   ├── shipper-config/                # .shipper.toml parsing/validation
│   ├── shipper-types/                 # Shared types (Plan, ExecutionState, Receipt, events)
│   ├── shipper-registry/              # Registry HTTP clients
│   ├── shipper-cargo-failure/         # Cargo error classification (ErrorClass patterns)
│   ├── shipper-duration/              # Human-readable duration parsing
│   ├── shipper-encrypt/               # State encryption (optional)
│   ├── shipper-output-sanitizer/      # ANSI strip + token redaction
│   ├── shipper-retry/                 # Retry/backoff strategies
│   ├── shipper-sparse-index/          # Sparse index protocol client
│   └── shipper-webhook/               # Webhook delivery
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

## Three-crate product shape (#95)

```
shipper (install face)
  -> shipper-cli (CLI adapter; pub fn run())
       -> shipper-core (engine; stable embedding surface)
```

- Users type `cargo install shipper --locked` — the `shipper` crate carries the `shipper` binary, which forwards to `shipper_cli::run()`.
- Embedders add `shipper-core` to their `Cargo.toml` — no `clap`, no `indicatif`, no progress rendering pulled into their dep graph.
- The `shipper` library surface re-exports a curated subset of `shipper-core` (`engine`, `plan`, `types`, `config`, `state`, `store`) for drivers that prefer the product name. Engine internals reach through `shipper-core` directly.

All 13 workspace crates are published to crates.io. There is no separate "workspace-internal" tier — when a concern needs ownership, it lives as a module inside an owner crate (see [architecture.md](architecture.md)).

## `crates/shipper-core` module map

```
crates/shipper-core/src/
├── lib.rs                # Public library surface
├── engine/               # Plan/preflight/publish/resume execution
│   ├── mod.rs            # Top-level engine entry points (run_preflight, run_publish, run_resume, run_rehearsal)
│   ├── plan_yank.rs      # Reverse-topological yank planning
│   ├── fix_forward.rs    # Fix-forward planning for compromised releases
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
├── ops/                  # I/O primitives (layer 1)
│   ├── auth/             # Token resolution + OIDC detection (oidc.rs)
│   ├── cargo/            # cargo subprocess invocation
│   ├── git/              # Git working-tree checks
│   ├── lock/             # File-based distributed lock
│   ├── process/          # Subprocess capture
│   └── storage/          # Storage backend trait + filesystem impl
├── config.rs             # Internal config helpers
├── git.rs                # Public facade over ops/git
├── encryption.rs         # State encryption (uses shipper-encrypt)
├── webhook.rs            # Webhook event emission (uses shipper-webhook)
├── types.rs              # Crate-internal type aliases + re-exports of shipper-types
├── property_tests.rs     # proptest harnesses
└── stress_tests.rs       # Long-running validation
```

## `crates/shipper-cli` module map

```
crates/shipper-cli/src/
├── lib.rs                # pub fn run() — argparse, subcommand dispatch
├── main.rs               # 3-line wrapper: shipper_cli::run()
└── output/               # Human output: progress bars, snapshots, formatting
    └── progress/         # Terminal progress rendering
```

## `crates/shipper` module map

```
crates/shipper/
├── src/
│   ├── lib.rs            # Curated re-export of shipper-core
│   └── bin/
│       └── shipper.rs    # 3-line wrapper: shipper_cli::run()
├── README.md             # Product landing page
└── tests/                # Integration tests exercising the façade
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
- **BDD scenarios** — `features/` + `crates/shipper-cli/tests/bdd_*.rs`
- **Fuzz targets** — `fuzz/fuzz_targets/`
- **Snapshots** — `insta`
- **Property tests** — `proptest`
- Tests touching env vars or filesystem use `#[serial]` from `serial_test`
- Registry interactions use `tiny_http` mock servers — never hit real registries
