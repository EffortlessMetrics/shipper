# SHIPPER-SPEC-0002: Release Readiness Proof

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/release-readiness-proof.md
Linked issues: #109, #195
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper 0.4.0 needs a reusable release-readiness contract before #195 produces
the release artifact. The artifact should answer whether the workspace is ready
to publish and what evidence supports that answer. It must not be a checklist of
intent; it records command results, publish dry-run evidence, advisory signals,
known carry-over, and sign-off for a specific version.

This spec defines what a release-readiness proof must contain. The command
ordering and PR mechanics live in `plans/0.4.0/release-readiness-proof.md`.

## Behavior Contract

A release-readiness proof for a Shipper release candidate must:

- identify the version under review
- identify the commit SHA under review
- record the Shipper plan id used for the proof
- include the preflight result or explain why preflight could not be completed
- include policy-report status and artifact links
- include lint, no-panic, file-policy, and doc-contract state
- include ripr advisory state
- include mutation state when mutation evidence is requested for the release
- include a dry-run publish table for every publishable crate
- identify the authoritative plan order used for crate dry-runs
- include cargo publish dry-run evidence for each crate
- link CI runs and uploaded artifacts when CI is part of the evidence packet
- list known carry-over explicitly
- include operator sign-off

The proof must distinguish observed evidence from planned future work. It must
not tag, publish, or imply that a release was published.

## Non-Goals

- Publishing crates.
- Creating a git tag or GitHub release.
- Implementing registry reconciliation.
- Promoting registry reconciliation or interruption-resume claims.
- Replacing policy ledgers with prose.
- Capturing every cargo output line in the readiness document; detailed logs may
  be linked or summarized.

## Required Evidence

The readiness proof must include results from these gates:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo test --workspace --doc`
- `cargo check --workspace`
- `cargo audit`
- `cargo doc --workspace --no-deps --document-private-items`
- `cargo test -p shipper-cli --test bdd_publish`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`

The readiness proof must also include cargo publish dry-run evidence for every
publishable crate in the plan order reported by `cargo run -p shipper -- plan`.

## Acceptance Examples

- A readiness document with a version but no commit SHA is incomplete.
- A readiness document that says "policy passed" without linking or summarizing
  `cargo xtask policy-report` is incomplete.
- A dry-run table that omits a publishable crate is incomplete.
- A readiness document that lists Reconcile as implemented before registry
  reconciliation behavior exists is invalid.
- A support-tier promotion from planned to stable is invalid unless the
  readiness artifact exists and the proof commands passed or the exception is
  explicitly receipted.
- If `shipper plan` reports an order different from a hand-written plan, the
  `shipper plan` output is authoritative.

## Test Mapping

This spec is proven by the #195 release-readiness PR, not by unit tests alone.
The proof maps to:

- formatting, lint, test, audit, documentation, BDD, and policy commands
- `cargo run -p shipper -- plan` for plan id and topological order
- per-crate `cargo publish --dry-run -p <crate>` results
- `docs/release/0.4.0-readiness.md` for the human-readable evidence packet
- `target/policy/policy-report.{md,json}` and
  `target/policy/doc-contracts-report.{md,json}` for machine-readable policy
  evidence

## Implementation Mapping

`plans/0.4.0/release-readiness-proof.md` sequences the release-readiness work.
The first consumer is #195, which must produce
`docs/release/0.4.0-readiness.md`.

## CI Proof

CI contributes evidence when it runs the same gates and uploads policy
artifacts. CI output is not enough by itself: the readiness document must record
which CI run, commit, plan id, and dry-run table belong to the release
candidate.

## Promotion Rule

The `0.4.0 release readiness proof` support-tier entry may move from
`planned until #195` to `stable` only after `docs/release/0.4.0-readiness.md`
exists and records the required evidence.

Release-readiness proof does not by itself promote adjacent release-closure
claims. Registry reconciliation and interruption-resume support-tier rows must
name their own specs, tests, and artifacts. Current proof lives in
`docs/status/SUPPORT_TIERS.md`: ambiguous publish reconciliation is stable, and
resume under live runner interruption is stable/internal against fake
Cargo/mock registry proof surfaces.

## Open Questions

- Whether future release-readiness proof should also emit a JSON artifact.
- Whether mutation evidence should be mandatory for every release candidate or
  only when requested by release policy.
