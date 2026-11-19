# Clippy Policy

This document describes the workspace lint policy for `shipper`. The authoritative Clippy ledger is `policy/clippy-lints.toml`; this document explains the rationale and operating rules. `[workspace.lints]` in `Cargo.toml` is the executable configuration.

## Goals

1. Keep the codebase warning-free under CI (`cargo clippy -- -D warnings`).
2. Activate new lints deliberately, with a policy record, not opportunistically.
3. Track planned lints against the MSRV that enables them.
4. Reject fabricated or unavailable lint names before they can become policy.
5. Never use broad category allows or test carveouts.

## Active Lints

The following entries are active in the policy ledger and workspace lint configuration:

| Lint | Level | Class | Reason |
|---|---|---|---|
| `rust::unsafe_code` | forbid | unsafe-memory | Shipper has no unsafe code |
| `clippy::dbg_macro` | deny | hygiene | Debug macros are not a reviewable diagnostics path |
| `clippy::todo` | deny | panic | TODO execution paths are not allowed |
| `clippy::unimplemented` | deny | panic | Unimplemented execution paths are not allowed |
| `clippy::same_length_and_capacity` | deny | correctness | Catch raw-parts reconstruction mistakes |
| `clippy::manual_ilog2` | warn | style | Prefer the standard integer logarithm helper |
| `clippy::decimal_bitwise_operands` | warn | correctness | Make bit masks visually inspectable |
| `clippy::needless_type_cast` | warn | style | Avoid stale numeric type drift |
| `clippy::manual_checked_ops` | warn | correctness | Prefer checked arithmetic over manual guards |
| `clippy::manual_take` | warn | style | Use the standard ownership helper |
| `clippy::unnecessary_trailing_comma` | warn | style | Keep format-macro calls clean |
| `clippy::disallowed_fields` | deny | boundary | Reject access to any fields configured as protected seams |
| `clippy::duration_suboptimal_units` | warn | style | Express durations using the largest available unit |

Warn-level entries are still blocking in CI because Clippy runs with `-D warnings`.

## Planned and Rejected Lints

There are currently no `[[planned]]` entries in `policy/clippy-lints.toml`. A future planned lint must name its minimum MSRV, expected fallout, owner, and activation condition before it is added.

`clippy::manual_pop_if` appeared in the original Rust 1.95 planning issue but is not a Clippy lint in Rust 1.95. It is deliberately absent. `rust::unknown_lints = "deny"` makes an unavailable or fabricated lint fail the activation change rather than becoming decorative policy.

## `disallowed_fields` Protected Seams

`clippy::disallowed_fields` is active at `deny`. `clippy.toml` does not currently configure any `disallowed-fields` entries, so the lint has no protected field list to enforce yet.

Candidate seams remain:

- `state.json` / `events.jsonl` projection fields
- receipt summary internals
- plan ID / workspace fingerprint fields
- registry token / auth policy surfaces
- readiness verification outcomes
- ambiguous publish reconciliation state
- encrypted state internals
- output sanitizer internals

Adding a protected field is a separate reviewed policy change. It must identify the owning abstraction, migrate legitimate callers first, add a negative fixture proving direct access is rejected, and update both `clippy.toml` and the ledger. The lint being active is not evidence that any particular seam is already protected.

## Suppression Policy

All suppressions must use `#[expect(clippy::lint_name, reason = "...")]`, not bare `#[allow(clippy::...)]`. Suppressions with no reason are not permitted.

Debt suppressions are receipted in `policy/clippy-debt.toml` with owner and expiry. Exceptions with business justification are receipted in `policy/clippy-exceptions.toml`.

The `cargo xtask check-clippy-exceptions` command enforces that exceptions have owner, reason, and expiry fields, and that no exception has expired.

## MSRV Alignment

The `clippy.toml` file carries `msrv = "<current MSRV>"`. The `policy/clippy-lints.toml` file carries the same value. Both must agree with `[workspace.package] rust-version` in `Cargo.toml`. The `cargo xtask check-lint-policy` command verifies alignment and checks configured workspace Clippy lints against the ledger.

## CI Behavior

CI runs `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`. Any active warn- or deny-level lint that fires therefore breaks the required Rust lane. Planned lints must not be placed in `[workspace.lints.clippy]` until their activation PR has measured and resolved the fallout. There are no per-PR test carveouts.

## Cognitive Complexity

The `clippy.toml` file carries `cognitive-complexity-threshold = 40`. Functions that exceed this threshold must be refactored rather than hidden behind a broad lint allow.
