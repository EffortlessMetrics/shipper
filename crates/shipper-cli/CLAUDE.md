# CLAUDE.md

This file provides agent-specific guidance for working in crate shipper-cli.

## Scope

- Crate: shipper-cli
- Path: crates/shipper-cli
- Workspace root: h:\Code\Rust\shipper
- Primary entry: src/main.rs

## Useful commands

```bash
cargo check -p shipper-cli
cargo test -p shipper-cli
cargo test -p shipper-cli --all-features
cargo fmt -p shipper-cli
cargo clippy -p shipper-cli --all-targets --all-features -- -D warnings
```

## Context

- Keep changes small and targeted to the crate’s existing abstractions.
- Preserve public API compatibility unless the request explicitly asks for breaking changes.
- When touching serialization or state formats, update tests and related snapshots in the same crate.
- Prefer using existing fixtures and helpers rather than introducing inline test data.

## GitHub Actions recipe

`src/ci.rs` owns the GitHub Actions YAML step sequence printed by `ci
github-actions`. Keep it suitable for `jobs.publish.steps`, with one literal
checkout-relative state path shared by the quoted publish invocation and hidden
evidence upload. The fresh recipe refuses retained authoritative evidence; it
does not choose an earlier artifact or restore mutable execution state. The
documented wrapper uses `preflight --preflight-only` so its audit sidecar does
not trigger that refusal. Its workspace is at the checkout root.

The contract tests in `tests/e2e_publish/ci_github_actions.rs` consume the actual
generated shell step and the documented wrapper. Keep structural/actionlint
proof separate from local fake-Cargo execution and from hosted artifact transfer.
Other CI providers retain their existing generator contracts.

For full workspace guidance, see [../../CLAUDE.md](../../CLAUDE.md).
