# Copilot instructions for shipper

This file collects repository-specific guidance for automated assistants (Copilot/CLI agents) to work effectively in this Rust workspace.

---

## Quick summary

- Repository is a Rust workspace with two members: `crates/shipper` (library) and `crates/shipper-cli` (binary/CLI).
- The library builds a deterministic `ReleasePlan` and the CLI (shipper) runs commands that build plans, run preflight checks, and execute/resume publishes with retry/backoff and persistence.

---

## Build, test, and lint commands

From the repository root:

- Build (debug):
  - `cargo build`
- Build (release):
  - `cargo build --release`
- Install the CLI locally (recommended for manual testing):
  - `cargo install --path crates\\shipper-cli --locked`
- Run the CLI without installing (useful during development):
  - `cargo run -p shipper-cli -- <command>`  (e.g. `cargo run -p shipper-cli -- plan`)

Shipper CLI common commands (after installing or via `cargo run -p shipper-cli`):
- `shipper plan`
- `shipper preflight`
- `shipper publish`
- `shipper resume`
- `shipper status`
- `shipper doctor`

Tests / single-test usage:
- Run all tests (workspace): `cargo test`
- Run tests for a single package: `cargo test -p shipper` or `cargo test -p shipper-cli`
- Run a specific test by name (substring): `cargo test -p shipper some_test_name`
- Run an exact test name: `cargo test -p shipper some_test_name -- --exact`
- Run an integration test binary: `cargo test --test <testname> -p shipper-cli`

Formatting & linting:
- Format code: `cargo fmt --all`
- Check formatting (CI): `cargo fmt --all -- --check`
- Run clippy (recommended flags for CI/local):
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`

Toolchain:
- The workspace declares `rust-version = "1.92"` in `Cargo.toml`; use the matching toolchain (rustup) when reproducibility is required.

CI templates:
- See `templates/github-trusted-publishing.yml` and `templates/gitlab-publish.yml` for example CI steps (they show installing `shipper-cli` and running `shipper publish` on tagged pushes).

---

## High-level architecture (big picture)

- Workspace layout:
  - `crates/shipper` — core library exposing modules: `auth`, `cargo`, `engine`, `git`, `plan`, `registry`, `state`, `types`.
  - `crates/shipper-cli` — binary that parses CLI args and calls into the library (builds `ReleaseSpec`/`RuntimeOptions` then runs plan/preflight/publish/resume/status/doctor flows).

- Primary flow:
  1. Build a deterministic `ReleasePlan` from the workspace manifest.
  2. Optionally run preflight checks (git cleanliness, publishability, ownership, registry reachability).
  3. Execute the plan: publish crates one-by-one using `cargo publish -p <crate>` with retry/backoff and verification of registry visibility.
  4. Persist progress to disk (`.shipper/state.json`) and write a `receipt.json` for CI/audit.

- Persistence & audit:
  - By default state and receipts are written under `.shipper` in the workspace root. Use `--state-dir <path>` to change this location.

- Registry & auth:
  - The project performs explicit registry checks (version existence and optional owners checks) and resolves tokens from the standard places: `CARGO_REGISTRY_TOKEN`, `CARGO_REGISTRIES_<NAME>_TOKEN`, or `$CARGO_HOME/credentials.toml`.

- Error handling and retries:
  - The engine applies exponential backoff with jitter for retryable failures and verifies registry state before treating a step as failed (see `docs/failure-modes.md`).

---

## Key conventions and repository-specific patterns

- Workspace crate split: prefer library logic in `crates/shipper` and keep the CLI thin in `crates/shipper-cli`.
- State files: `.shipper/state.json` (resumable state) and `.shipper/receipt.json` (machine-readable receipt for CI/auditing). Prefer `--state-dir` for CI artifact storage.
- Token resolution: treat tokens as opaque strings; resolve from `CARGO_REGISTRY_TOKEN`, `CARGO_REGISTRIES_<NAME>_TOKEN`, or `CARGO_HOME` credentials.
- Unsafe code: the workspace Cargo.toml sets `unsafe_code = "forbid"` (see `[workspace].lints.rust`), so avoid adding unsafe blocks.
- Tests:
  - Many tests use `serial_test` and are intentionally run serially (tests may mutate global env or filesystem); use `#[serial]` in tests that need isolation.
  - Tests mock registry interactions (e.g. `tiny_http`) — prefer local HTTP mocks in tests rather than hitting real registries.
  - Snapshot testing uses `insta` in dev-dependencies.
- CLI flags commonly used during development/debugging:
  - `--manifest-path <path>` (defaults to `Cargo.toml`)
  - `--state-dir <path>` to relocate state/receipts
  - `--packages` to restrict to specific packages
  - `--skip-ownership-check` and `--strict-ownership` to control owners preflight behavior
  - `--no-verify` to pass `--no-verify` to `cargo publish`
- Selecting single-package operations: when running cargo commands in this workspace, prefer `-p <package>` to scope operations (e.g., `cargo test -p shipper` or `cargo run -p shipper-cli`).

---

## Where to look for more details

- README.md (root) — quick start, commands, and install instructions.
- `docs/failure-modes.md` — notes on partial publishes, ambiguous timeouts, rate limiting, and CI cancellations.
- `templates/` — example CI workflows for GitHub/GitLab showing how this repo is expected to be used in release pipelines.
- `crates/shipper/src` — implementation entry points and module breakdown.

---

## AI assistant / Copilot notes

- No repository-specific Copilot/assistant instruction files were found (e.g., `CLAUDE.md`, `.cursorrules`, `AGENTS.md`, `.windsurfrules`, `CONVENTIONS.md`).
- This file should be the primary source of repo-specific guidance for future Copilot sessions.

---

If anything should be expanded (more examples, CI-specific notes, or per-crate testing guidance), say which area to expand and a short rationale.
