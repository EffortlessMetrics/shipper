# Tech Stack

> Snapshot of dependencies and conventions. Versions drift — check `Cargo.toml` files for current pins.

## Language & toolchain

- **Rust edition 2024**
- **MSRV: 1.92**
- **Resolver v3**
- `unsafe_code = "forbid"` workspace-wide — no `unsafe` blocks anywhere

## Key runtime dependencies

| Crate | Version (rough) | Purpose |
|---|---|---|
| `clap` (derive) | 4.x | CLI parsing in `shipper-cli` |
| `clap_complete` | 4.x | Shell completion generation |
| `anyhow` | 1.x | Error handling across library and CLI |
| `thiserror` | 2.x | Typed error definitions |
| `serde` / `serde_json` / `serde_with` | 1.x / 1.x / 3.x | All structured I/O (config, state, receipts, events) |
| `cargo_metadata` | 0.23.x | Reading workspace manifests |
| `reqwest` | 0.13.x (blocking + json + rustls) | Registry HTTP API + webhook delivery |
| `chrono` | 0.4.x | Timestamps in events and receipts |
| `humantime` | 2.x | Human-readable duration parsing/formatting |
| `tokio` | 1.x | Async primitives (limited use; most logic is sync) |
| `toml` | 1.x | `.shipper.toml` parsing |
| `console` / `indicatif` | 0.16.x / 0.18.x | TTY output + progress bars |
| `sha2` | 0.10.x | Plan ID computation |
| `aes-gcm` / `hmac` | 0.10 / 0.13 | State encryption (optional) |
| `which` | 8.x | Tool discovery (`cargo`, `git`) |
| `dirs` / `gethostname` / `hex` / `rand` | misc | Standard utilities |

## Test & dev dependencies

| Crate | Purpose |
|---|---|
| `insta` (with `yaml` feature) | Snapshot tests |
| `proptest` | Property-based tests |
| `serial_test` | Serializing tests that mutate env or global state |
| `tiny_http` | Mock registry servers for tests (real registries are never hit) |
| `tempfile` | Filesystem isolation in tests |
| `temp-env` | Scoped environment-variable mutation |
| `cargo-fuzz` | Fuzzing harness for state loading + token resolution |

## Build & install

See [../CLAUDE.md](../CLAUDE.md) for the canonical command list. Quick reference:

```bash
cargo build --release            # production binary (LTO + strip)
cargo install --path crates/shipper --locked
cargo test                       # workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

## CI

GitHub Actions in `.github/workflows/`:

- `ci.yml` — workspace test/lint/format/MSRV
- `release.yml` — tag-triggered publish via Shipper itself (the dogfooding train)
- Additional workflows for fuzz, security, docs as needed

## Conventions

- **Token handling.** Opaque strings, resolved per Cargo convention: `CARGO_REGISTRY_TOKEN` → `CARGO_REGISTRIES_<NAME>_TOKEN` → `$CARGO_HOME/credentials.toml`. Never logged. Sanitized in receipts via `shipper-output-sanitizer`.
- **Atomic file writes.** All state files use write-temp + fsync + rename + fsync-parent.
- **Schema versioning.** `state.json`, `receipt.json`, and `.shipper.toml` carry schema versions for future migration.
- **Determinism.** `BTreeSet` / `BTreeMap` over `HashSet` / `HashMap` where iteration order is observable.
- **Error classification.** `ErrorClass::{Retryable, Permanent, Ambiguous}`. Only retryable triggers backoff. Ambiguous should reconcile against registry truth ([#99](https://github.com/EffortlessMetrics/shipper/issues/99) tracks closing this).
- **Event emission.** Every state transition emits one event in `events.jsonl`. Adding a new code path means adding the corresponding event variant in `shipper-types`.
- **No real registry calls in tests.** Mock with `tiny_http`.
