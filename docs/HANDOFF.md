# Shipper Full Handoff

Date: 2026-02-16
Prepared at: 2026-02-16T07:30:02-05:00

## 1) Repository Snapshot

- Repository: `shipper` (Rust workspace)
- Current branch: `next-pr`
- Upstream tracking: `origin/next-pr` (`ahead 2`, `behind 5`)
- HEAD:
  - Full SHA: `577246dbd1b793f9f732561d926592a5f3d6e53c`
  - Short SHA: `577246d`
  - Commit date: `2026-02-16T05:32:52-05:00`
  - Subject: `fix: stabilize timeout, redaction, and Uploaded semantics (#3)`
- Working tree state: dirty (1 file modified)
  - `crates/shipper-cli/Cargo.toml`
  - Diff: `shipper = { path = "../shipper" }` -> `shipper = { path = "../shipper", version = "0.2.0" }`

## 2) Toolchain / Environment Snapshot

- `cargo 1.92.0 (344c4567c 2025-10-21)`
- `rustc 1.92.0 (ded5c06cf 2025-12-08)`
- Workspace config (`Cargo.toml`) uses:
  - Edition: `2024`
  - Rust version: `1.92`
  - Resolver: `3`

## 3) Current Product Scope (v0.2.0)

Shipper is a reliability layer around `cargo publish` for Rust workspaces.

Core value:
- deterministic publish planning
- preflight verification
- retry/backoff behavior
- readiness verification (API/index/both)
- resumable execution with persisted state
- evidence-first receipts and append-only event logs
- optional parallel publishing of independent dependency levels

Primary crates:
- `crates/shipper` (library engine)
- `crates/shipper-cli` (CLI binary: `shipper`)

## 4) Architecture and Important Modules

Primary publish flow:
- `plan::build_plan` -> `engine::run_preflight` -> `engine::run_publish` -> `engine::run_resume`

Key modules:
- `crates/shipper/src/plan.rs`
  - workspace metadata discovery
  - publishability filtering
  - deterministic topological ordering
  - dependency-level grouping for parallel mode
- `crates/shipper/src/engine.rs`
  - sequential publish engine
  - policy effects (`safe`/`balanced`/`fast`)
  - preflight finishability (`Proven`/`NotProven`/`Failed`)
  - retry classification and backoff
  - Uploaded-state resume semantics
- `crates/shipper/src/engine_parallel.rs`
  - wave/level parallel publishing
  - shared state/event coordination via `Arc<Mutex<...>>`
  - per-package timeout support
- `crates/shipper/src/registry.rs`
  - registry API existence checks
  - ownership checks
  - sparse index visibility checks
  - readiness polling with backoff/jitter
- `crates/shipper/src/cargo.rs`
  - shelling out to cargo publish/dry-run
  - output tail capture
  - sensitive value redaction
  - timeout handling for per-package publish
- `crates/shipper/src/state.rs`
  - atomic state/receipt persistence
  - schema version validation and receipt migration (`v1` -> `v2`)
- `crates/shipper/src/events.rs`
  - append-only JSONL event stream
- `crates/shipper/src/lock.rs`
  - lock file with stale lock timeout/override support
- `crates/shipper/src/config.rs`
  - `.shipper.toml` parsing and validation
  - CLI override merge strategy
- `crates/shipper-cli/src/main.rs`
  - command wiring, formatters, diagnostics, CI snippet generation, clean/config subcommands

## 5) Runtime Artifacts and Operator Paths

Default state directory: `.shipper/`

Files:
- `.shipper/state.json` (resume state)
- `.shipper/receipt.json` (audit receipt, schema `shipper.receipt.v2`)
- `.shipper/events.jsonl` (event log)
- `.shipper/lock` (concurrency lock)

## 6) Validation Performed in This Session

All commands run from workspace root: `h:\Code\Rust\shipper`.

Quality gates:
- `cargo fmt --check` -> pass
- `cargo clippy --workspace -- -D warnings` -> pass
- `cargo test --workspace` -> pass

Test totals observed:
- `shipper` lib/unit/property tests: `263 passed`
- `shipper-cli` unit tests: `13 passed`
- `shipper-cli` e2e tests: `20 passed`
- doc tests: `0`
- Total observed pass count: `296`

Additional checks:
- TODO/FIXME debt scan via `rg` in `crates/`, `docs/`, and `README.md`: no matches

## 7) Documentation / Release State

Docs present and populated:
- `README.md`
- `docs/configuration.md`
- `docs/preflight.md`
- `docs/readiness.md`
- `docs/failure-modes.md`
- `RELEASE_NOTES_v0.2.0.md`
- `RELEASE_CHECKLIST_v0.2.0.md`

Release checklist currently still has unchecked publish/release steps (tag/push/publish/release creation).

## 8) Risks, Gaps, and Things to Watch

1. Branch divergence risk:
- `next-pr` is currently behind remote by 5 commits and ahead by 2.
- Merge/rebase decision should happen before any release/publish activity.

2. Uncommitted change present:
- `crates/shipper-cli/Cargo.toml` has a dependency pin change.
- Decide whether to keep/commit or revert before branching/release.

3. Potential doc drift:
- `RELEASE_CHECKLIST_v0.2.0.md` test counts are stale versus current observed totals.

4. Potential implementation footgun:
- In `crates/shipper/src/git.rs`, `git describe` includes an argument string `2>/dev/null`.
- This looks like shell redirection text passed as a raw git argument, not actual stderr redirection.
- Tag detection should be validated on a tagged commit.

## 9) Suggested Immediate Next Actions

1. Resolve branch divergence:
- `git fetch`
- rebase/merge `next-pr` onto latest `origin/next-pr` (or reset workflow if team prefers)

2. Resolve the local manifest change:
- either commit `crates/shipper-cli/Cargo.toml` version pin intentionally
- or drop it if it was accidental

3. If preparing release:
- update checklist values (especially test totals)
- ensure release tag and publish steps are executed in order

4. If preparing another engineering pass:
- add a focused test for real `git describe --tags --exact-match` behavior in `git.rs`
- patch tag detection if needed

## 10) Quick Operator Command Set

Planning / safety:
- `shipper plan`
- `shipper preflight`

Publish / recovery:
- `shipper publish`
- `shipper resume`

Diagnostics:
- `shipper status`
- `shipper doctor`
- `shipper inspect-events`
- `shipper inspect-receipt`

State hygiene:
- `shipper clean`
- `shipper clean --keep-receipt`

Config:
- `shipper config init`
- `shipper config validate`

