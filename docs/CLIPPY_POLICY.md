# Clippy Policy

This document describes the Clippy lint policy for `shipper`. The authoritative ledger is `policy/clippy-lints.toml`; this document explains the rationale and operating rules.

## Goals

1. Keep the codebase warning-free under CI (`RUSTFLAGS=-Dwarnings`, `cargo clippy -- -D warnings`).
2. Activate new lints deliberately, with a policy record, not opportunistically.
3. Track planned lints against the MSRV that enables them.
4. Never use broad category allows or test carveouts.

## Active Lints

The following lints are active at the workspace level in `Cargo.toml`:

| Lint | Level | Class | Reason |
|---|---|---|---|
| `rust::unsafe_code` | forbid | unsafe-memory | shipper has no unsafe code |
| `clippy::dbg_macro` | deny | hygiene | Debug macros are not a reviewable diagnostics path |
| `clippy::todo` | deny | panic | TODO execution paths are not allowed |
| `clippy::unimplemented` | deny | panic | Unimplemented execution paths are not allowed |

## Planned Lints (MSRV-gated)

These lints are tracked in `policy/clippy-lints.toml` under `[[planned]]`. They activate when the MSRV reaches the stated minimum.

| Lint | Level | Min MSRV | Reason |
|---|---|---|---|
| `clippy::same_length_and_capacity` | deny | 1.94 | Catch raw-parts reconstruction mistakes |
| `clippy::manual_ilog2` | warn | 1.94 | Prefer standard integer log helper |
| `clippy::decimal_bitwise_operands` | warn | 1.94 | Make bit masks visually inspectable |
| `clippy::needless_type_cast` | warn | 1.94 | Avoid stale numeric type drift |
| `clippy::manual_checked_ops` | warn | 1.95 | Prefer checked arithmetic over manual guards |
| `clippy::manual_take` | warn | 1.95 | Use standard ownership helper |
| `clippy::manual_pop_if` | warn | 1.95 | Use predicate-and-pop collection APIs |
| `clippy::duration_suboptimal_units` | warn | 1.95 | Make durations legible |
| `clippy::unnecessary_trailing_comma` | warn | 1.95 | Keep format macro calls clean |
| `clippy::disallowed_fields` | deny | 1.95 | Ban direct field access across protected seams (pending seam configuration) |

## `disallowed_fields` Protected Seams

`disallowed_fields` is held in planned status until the following seams are explicitly configured:

- `state.json` / `events.jsonl` projection fields
- Receipt summary internals
- Plan ID / workspace fingerprint fields
- Registry token / auth policy surfaces
- Readiness verification outcomes
- Ambiguous publish reconciliation state
- Encrypted state internals
- Output sanitizer internals

## Suppression Policy

All suppressions must use `#[expect(clippy::lint_name, reason = "...")]`, not bare `#[allow(clippy::...)]`. Suppressions with no reason are not permitted.

Debt suppressions are receipted in `policy/clippy-debt.toml` with owner and expiry. Exceptions with business justification are receipted in `policy/clippy-exceptions.toml`.

The `cargo xtask check-clippy-exceptions` command enforces that exceptions have owner, reason, and expiry fields, and that no exception has expired.

## MSRV Alignment

The `clippy.toml` file carries `msrv = "<current MSRV>"`. The `policy/clippy-lints.toml` file carries `msrv = "<current MSRV>"`. These three values must agree with `[workspace.package] rust-version` in `Cargo.toml`. The `cargo xtask check-lint-policy` command verifies alignment.

## CI Behavior

CI runs `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`. Since `RUSTFLAGS=-Dwarnings` is also set, any warn-level lint that fires breaks CI. Planned lints must therefore not be activated until the code is clean, or every warning must be fixed in the activation PR. There are no per-PR carveouts.

## Cognitive Complexity

The `clippy.toml` carries `cognitive-complexity-threshold = 40`. Functions that exceed this threshold must be refactored, not suppressed with an allow.
