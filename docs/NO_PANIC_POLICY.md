# No-Panic Policy

This document describes the no-panic discipline for `shipper`. The authoritative state is `policy/no-panic-baseline.toml` and `policy/no-panic-allowlist.toml`; this document explains the rationale and operating rules.

## Goals

1. Eliminate unintentional `unwrap`, `expect`, `panic!`, `unreachable!`, and similar panic-family calls from production code paths.
2. Track exactly what panic-family calls exist today so new debt cannot be added invisibly.
3. Allow necessary panics (test setup, true invariant assertions) by explicit receipt.
4. Prevent "one allowed unwrap hides ten unrelated unwraps" by keying allowlist entries to exact call-site identity.

## Policy Mode

The policy operates in `no-new-debt` mode. Existing panic-family calls that existed when the baseline was established are receipted in `policy/no-panic-baseline.toml`. Any call not in the baseline is a policy violation.

## Panic-Family Shapes Tracked

| Family | Members |
|---|---|
| `unwrap` | `.unwrap()`, `.unwrap_or_else(...)` that discard the `None`/`Err` |
| `expect` | `.expect("...")` |
| `panic` | `panic!()`, `panic_any()` |
| `unreachable` | `unreachable!()` |
| `todo` | `todo!()` (also denied by Clippy) |
| `unimplemented` | `unimplemented!()` (also denied by Clippy) |
| `index` | Unchecked slice indexing `slice[i]`, `map[k]` where bounds failure panics |

## Matching Key

Each baseline entry is identified by exact shape, not coarse file + family:

```toml
path = "crates/shipper-core/src/publish/engine.rs"
family = "unwrap"
selector_kind = "method_call"
selector_callee = "unwrap"
snippet = "state.packages.get(&name).unwrap()"
count = 1
```

This means one allowed `unwrap` does not mask unrelated calls in the same file.

## Allowlist vs Baseline

- **`policy/no-panic-allowlist.toml`**: Permanent receipts. Calls that are genuinely invariant assertions (truly cannot fail given the surrounding logic) and are owned indefinitely.
- **`policy/no-panic-baseline.toml`**: Debt snapshot. Calls that exist today but are not yet converted. The baseline is frozen and may only shrink; it cannot grow.

The baseline is marked `linguist-generated=true` in `.gitattributes` to indicate it is machine-maintained.

## Commands

```bash
# Check that no new debt has been added since the baseline.
cargo xtask check-no-panic-family

# Regenerate the baseline (only in PR 8; do not run in other PRs).
cargo xtask no-panic baseline --reset

# Report current policy state including panic-family debt.
cargo xtask policy-report
```

## Permitted Patterns

These patterns are acceptable in production code:

| Pattern | When acceptable |
|---|---|
| `.expect("invariant: ...")` with a note explaining the invariant | Known-impossible failure, documented |
| `unreachable!("exhaustive match: ...")` | Match arm that the type system guarantees is unreachable |
| Test `unwrap()` and `expect()` | Test setup/assertion only; excluded from production-code baseline |

## Prohibited Patterns

| Pattern | Why prohibited |
|---|---|
| `.unwrap()` on `Option`/`Result` in production code without allowlist receipt | Silent panic on unexpected input |
| `.expect("")` with empty reason | Provides no diagnostic value |
| `todo!()` in non-test code | Creates a live crash path (also denied by Clippy) |
| `unimplemented!()` in non-test code | Creates a live crash path (also denied by Clippy) |
| Bare panic! for error propagation | Use `anyhow::bail!` or typed errors instead |

## Release Proof

The release workflow calls `cargo xtask check-no-panic-family` as a release gate. A regression in the no-panic surface blocks publication.

## Critical Paths

The following paths are treated as highest-priority for no-panic cleanup because they handle real registry and state operations:

- `shipper-core` publish engine (`crates/shipper-core/src/engine/`)
- Ambiguous-publish reconciliation (`crates/shipper-core/src/engine/reconcile.rs`)
- State store and event log (`crates/shipper-core/src/state/`)
- Token resolution and sanitization (`crates/shipper-output-sanitizer/`, `crates/shipper-encrypt/`)
- Registry readiness verification (`crates/shipper-registry/`, `crates/shipper-sparse-index/`)
