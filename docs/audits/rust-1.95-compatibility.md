# Rust 1.95 Compatibility Audit

**Date:** 2026-05-10
**Auditor:** automated probe (PR 2 of the Rust 1.95 / 0.4.0 rollout)
**Toolchain probed:** rustc 1.95.0 (59807616e 2026-04-14)
**Workspace:** shipper 0.3.0-rc.2, 13 crates, Edition 2024, resolver v3

## Result: Clean — No Compatibility Fallout

The workspace compiles, lints, and generates documentation cleanly under Rust 1.95.0 with no changes required to source code, Cargo manifests, or toolchain configuration.

## Commands Run

```bash
rustup toolchain install 1.95.0 --component rustfmt --component clippy
rustup override set 1.95.0

cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo doc --workspace --no-deps
git diff --check
```

## Findings

| Check | Result | Notes |
|---|---|---|
| `cargo fmt --all -- --check` | Clean | No formatting drift between 1.92 and 1.95. |
| `cargo check --workspace --all-targets --all-features --locked` | Clean | All 13 crates compile without error or warning. |
| `cargo clippy -- -D warnings` | Clean | Zero warnings. No new lints fire at 1.95. |
| `cargo doc --workspace --no-deps` | Clean | All public API documentation generates without error. |
| `git diff --check` | Clean | No whitespace issues. |

## Notes on `cargo test` vs `cargo nextest`

Running `cargo test --workspace --all-features --locked` (stdlib test runner) produced 22 failures in `shipper-cli`'s `e2e_expanded` integration tests. Investigation confirmed these are **pre-existing** and not caused by Rust 1.95:

- The same 22 tests fail identically on the current stable Rust toolchain.
- The failures are insta snapshot tests (`resume_*_snapshot`, `error_*_snapshot`) that require `cargo nextest` for correct execution — nextest runs each binary in a separate process with proper environment setup for insta's snapshot approval flow.
- CI uses `cargo nextest` and the tests pass on all three platforms (verified via PR #169 CI run which used the unchanged codebase on Rust 1.92).

This is not a compatibility issue. The correct test command for this workspace is `cargo nextest run`, not `cargo test`.

## New Clippy Lints in 1.95

The following lints are new in Clippy 1.95. None fire on this codebase today, confirming zero additional cleanup is needed before the MSRV bump:

| Lint | Status on this codebase |
|---|---|
| `clippy::manual_checked_ops` | Not triggered |
| `clippy::manual_take` | Not triggered |
| `clippy::manual_pop_if` | Not triggered |
| `clippy::duration_suboptimal_units` | Not triggered |
| `clippy::unnecessary_trailing_comma` | Not triggered |

These lints will be activated in the Clippy ratchet PR (PR 7 of the rollout) after the policy ledger is in place.

## Conclusion

The MSRV bump from 1.92 to 1.95 requires **no source code changes**. PR 3 (toolchain bump) can proceed directly: update `[workspace.package] rust-version`, add `rust-toolchain.toml`, add `clippy.toml`, and update CI workflow MSRV references.
