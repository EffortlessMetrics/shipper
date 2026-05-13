# SHIPPER-SPEC-0002: Release Readiness Proof

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Linked proposal: SHIPPER-PROP-0001
Linked ADRs: SHIPPER-ADR-0001
Linked plan: plans/0.4.0/release-readiness-proof.md
Linked issues: #195
Linked PRs:
Support-tier impact: 0.4.0 release readiness proof
Policy impact: policy-report and doc-contract reports

## Problem

Shipper releases should produce an evidence packet. A release candidate is not
ready because prose says it is ready; it is ready because plan, preflight,
policy, tests, dry-runs, advisory signals, CI, and carry-over are recorded in
one artifact.

## Behavior Contract

A release-readiness artifact must include:

- version
- commit SHA
- plan ID
- preflight result
- policy-report summary
- lint, no-panic, and file-policy state
- ripr advisory state
- mutation state when requested
- dry-run publish table
- known carry-over
- links to CI runs or artifacts when available
- sign-off and explicit publish/tag boundary

The artifact must distinguish local proof from CI-only proof. It must not claim
publication, tagging, deployment, or cross-platform success without evidence.

## Non-Goals

- Publishing crates.
- Creating tags or GitHub releases.
- Implementing registry reconciliation.
- Treating advisory signals as release blockers unless a policy promotes them.

## Required Evidence

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo test --workspace --doc`
- `cargo check --workspace`
- `cargo audit`
- `cargo doc --workspace --no-deps --document-private-items`
- `cargo test -p shipper-cli --test bdd_publish`
- `cargo xtask policy-report`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo publish --dry-run --workspace`

The exact crate loop and local platform notes belong in the implementation
plan, not this spec.

## Acceptance Examples

- If all local gates pass but cross-platform CI has not run, the artifact says
  CI proof is pending.
- If `cargo publish --dry-run -p <crate>` fails because unpublished prerelease
  workspace dependencies are not in crates.io, the artifact records that as a
  single-crate Cargo limitation and relies on the workspace dry-run proof.
- If fuzz smoke cannot run locally because of toolchain or DLL failures, the
  artifact records the gap and points to the CI lane or follow-up.
- If ownership checks are skipped locally, preflight may be evidence but not a
  `PROVEN` release sign-off.

## Test Mapping

- `cargo test --workspace --all-features`
- `cargo test -p shipper-cli --test bdd_publish`
- policy and release commands listed above

## Implementation Mapping

- `plans/0.4.0/release-readiness-proof.md`
- `docs/release/0.4.0-readiness.md`
- `docs/status/SUPPORT_TIERS.md`
- `.shipper-meta/goals/active.toml`

## CI Proof

PR CI should supply clean-tree, cross-platform, policy, audit, and any available
fuzz-smoke receipts. The readiness artifact should link those runs or name the
gap.

## Promotion Rule

The release-readiness claim may move from planned to stable only after the
readiness artifact exists and points to passing proof commands or documented
CI-only gaps.

## Open Questions

- Whether future readiness artifacts should also emit machine-readable JSON.
- Whether release rehearsal artifacts should be required before every RC tag.
