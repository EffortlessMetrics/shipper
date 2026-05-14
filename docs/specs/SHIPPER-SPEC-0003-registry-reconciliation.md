# SHIPPER-SPEC-0003: Registry Reconciliation

Status: implemented
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: post-0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0002-registry-truth-and-reconciliation.md
Linked specs:
Linked ADRs:
Linked plan:
Linked issues: #99, #102, #109
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

An ambiguous `cargo publish` result is not a normal retryable failure. Cargo may
have uploaded the crate before the process exited with an error. Retrying before
checking registry truth can attempt a duplicate publish or hide what actually
happened from the operator.

Shipper must close ambiguous publish outcomes against registry evidence before
retrying or resuming.

## Behavior Contract

When a cargo publish attempt exits with an ambiguous class, Shipper must:

1. Persist that the package outcome is ambiguous before taking follow-up action.
2. Emit a `publish_reconciling` event before querying registry truth.
3. Query registry truth using bounded sparse-index and/or registry API evidence,
   honoring the configured readiness method where applicable.
4. Produce exactly one reconciliation outcome:
   - `Published`
   - `NotPublished`
   - `StillUnknown`
5. Emit a `publish_reconciled` event with the outcome and structured evidence.
6. Persist the reconciliation outcome in execution state.
7. Apply the outcome:
   - `Published`: mark the package complete and do not re-upload.
   - `NotPublished`: allow retry policy to continue.
   - `StillUnknown`: stop before blind retry and require operator action.
8. Make resume honor persisted reconciliation state:
   - `Published` packages are skipped.
   - `NotPublished` packages are eligible for retry.
   - `StillUnknown` packages require an explicit operator reconciliation path.

Cargo stdout, stderr, and exit code are classification hints. They are not the
authoritative answer for publish outcome after ambiguity is detected.

## Non-Goals

- Publishing or tagging a release.
- Changing non-ambiguous retry behavior.
- Treating Cargo output text as registry truth.
- Promoting Reconcile support-tier claims before implementation proof exists.
- Replacing post-success readiness checks; reconciliation is the ambiguous
  failure branch, not the normal success path.
- Querying real registries in tests.

## Required Evidence

The implementation lane must produce evidence for:

- `Published`: registry shows the package version after an ambiguous cargo exit.
- `NotPublished`: bounded registry checks establish the version is absent.
- `StillUnknown`: registry checks cannot establish truth within the configured
  bounds.
- Resume with persisted `Published` skips upload.
- Resume with persisted `NotPublished` is retry-eligible.
- Resume with persisted `StillUnknown` does not blind-retry.
- Event logs contain `publish_reconciling` and `publish_reconciled` entries.
- State and events remain consistent with the events-as-truth invariant.

## Acceptance Examples

- Cargo exits 101, registry API shows the version, sparse index agrees:
  reconciliation outcome is `Published`.
- Cargo exits with a timeout, bounded registry checks complete and the version
  remains absent: reconciliation outcome is `NotPublished`.
- Cargo exits with a network failure, and both registry evidence paths are
  unavailable or inconclusive: reconciliation outcome is `StillUnknown`.
- Resume sees a persisted `Published` reconciliation outcome: the package is
  skipped without another `cargo publish`.
- Resume sees a persisted `StillUnknown` reconciliation outcome: Shipper stops
  for operator action instead of retrying.
- A test that asserts behavior from Cargo stderr alone is incomplete; registry
  evidence must decide the outcome.

## Test Mapping

Expected implementation proof:

- `cargo test -p shipper-types reconciliation`
- `cargo test -p shipper-core reconciliation`
- `cargo test -p shipper-core state`
- `cargo test -p shipper-cli --test bdd_publish`
- targeted resume tests for all three reconciliation outcomes
- policy/doc gates:
  - `cargo xtask check-doc-contracts --mode advisory`
  - `cargo xtask policy-report`

Tests must use mock registry surfaces, not real registries.

## Implementation Mapping

The implementation plan should split the lane into:

- reconciliation outcome types and events
- registry evidence collector
- ambiguous publish branch integration
- resume integration
- BDD and failure-mode tests
- support-tier and README claim promotion

The ADR for this spec should record that registry truth outranks Cargo process
output.

## CI Proof

CI should run the unit, integration, BDD, and policy gates for the implementation
PRs. Reconcile support-tier promotion is not valid until CI proves the outcome
state machine and resume behavior.

## Promotion Rule

`docs/status/SUPPORT_TIERS.md` kept ambiguous publish reconciliation as
`planned` until:

- this spec is accepted
- the registry-truth ADR is accepted
- the implementation plan exists
- the implementation was complete
- tests covered `Published`, `NotPublished`, `StillUnknown`, and resume behavior
- README and product docs were aligned with the proven tier

The current support tier is owned by `docs/status/SUPPORT_TIERS.md`, not this
spec. If future evidence weakens or expands the claim, update the support-tier
entry and proof commands first.

## Open Questions

- Which operator command or flag should clear a persisted `StillUnknown` state?
- Should reconciliation evidence be included in the final release receipt, a
  dedicated registry-truth artifact, or both?
- How should bounded polling parameters differ between crates.io and custom
  registries?
