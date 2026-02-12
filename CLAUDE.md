# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Build
cargo build                    # debug
cargo build --release          # release (LTO + strip)

# Run CLI during development (without installing)
cargo run -p shipper-cli -- <command>

# Install CLI locally
cargo install --path crates/shipper-cli --locked

# Tests
cargo test                                         # all workspace tests
cargo test -p shipper                              # library crate only
cargo test -p shipper-cli                          # CLI crate only
cargo test -p shipper some_test_name               # substring match
cargo test -p shipper some_test_name -- --exact    # exact match
cargo test --test cli_e2e -p shipper-cli           # integration tests only

# Lint & format
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Architecture

Rust workspace with two crates:

- **`crates/shipper`** — Core library. All domain logic lives here.
- **`crates/shipper-cli`** — Thin CLI binary. Parses args with clap, builds a `ReleaseSpec`/`RuntimeOptions`, then calls into the library.

Keep library logic in `crates/shipper`; keep `crates/shipper-cli` as a thin dispatch layer.

### Publishing Pipeline

The core flow is: **plan → preflight → publish → (resume if interrupted)**

1. **Plan** (`plan.rs`): Reads workspace via `cargo_metadata`, filters publishable crates, topologically sorts by intra-workspace dependencies (Kahn's algorithm with BTreeSet for determinism), generates a SHA256-based plan ID.
2. **Preflight** (`engine.rs`): Validates git cleanliness, registry reachability, dry-run, version existence, and optional ownership checks. Produces a `Finishability` assessment (Proven/NotProven/Failed).
3. **Publish** (`engine.rs`): Executes plan one crate at a time with retry/backoff. After each `cargo publish`, verifies registry visibility (API or sparse index) before proceeding. Persists `ExecutionState` to disk after every step for resumability.
4. **Resume**: Reloads state from `.shipper/state.json`, validates plan ID match, skips already-published packages, continues from first pending/failed.

### Key Abstractions

- **`StateStore` trait** (`store.rs`): Persistence abstraction for state/receipt/events. Currently filesystem-backed; designed for future cloud storage backends.
- **`Reporter` trait** (`engine.rs`): Pluggable output handler for publish/preflight progress.
- **`ErrorClass`** enum: Classifies failures as `Retryable` (HTTP 429, network), `Permanent` (auth, version conflict), or `Ambiguous` (upload may have succeeded despite client error). Only retryable errors trigger backoff retries.
- **`PublishPolicy`/`VerifyMode`/`ReadinessMethod`**: Configuration enums controlling safety vs speed tradeoffs.

### State Files

Written to `.shipper/` (configurable via `--state-dir`):
- `state.json` — resumable execution state (schema-versioned)
- `receipt.json` — audit receipt with evidence (stdout/stderr, exit codes, git context, environment fingerprint)
- `events.jsonl` — append-only event log
- `lock` — distributed lock preventing concurrent publishes

## Conventions

- **`unsafe_code = "forbid"`** is enforced workspace-wide. No unsafe blocks.
- Edition 2024, MSRV 1.92, resolver v3.
- Tests that mutate environment variables or filesystem use `#[serial]` from `serial_test` for isolation.
- Registry interactions in tests use `tiny_http` mock servers, never real registries.
- Snapshot tests use `insta`. Property-based tests use `proptest`.
- Token resolution follows Cargo conventions: `CARGO_REGISTRY_TOKEN` → `CARGO_REGISTRIES_<NAME>_TOKEN` → `$CARGO_HOME/credentials.toml`. Tokens are opaque strings, never logged.
- Configuration can be set via `.shipper.toml` in workspace root; CLI flags override config file values. Config sections: `[policy]`, `[verify]`, `[readiness]`, `[output]`, `[lock]`, `[retry]`, `[flags]`, `[parallel]`, `[registry]`. Ownership/git settings live in `[flags]`, not a separate `[preflight]` section.
- `config init` uses `-o`/`--output`; `config validate` uses `-p`/`--path`.
- `prefer_index` and `index_path` (readiness) are config-file-only settings with no CLI flags.
