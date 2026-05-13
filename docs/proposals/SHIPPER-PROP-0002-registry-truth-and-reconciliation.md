# SHIPPER-PROP-0002: Registry Truth and Reconciliation

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: post-0.4.0
Linked proposal:
Linked specs:
Linked ADRs:
Linked plan:
Linked issues: #99, #102, #109
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper's largest remaining safety gap is Reconcile. When `cargo publish`
returns an ambiguous outcome, the upload may have succeeded even though the
process result looks like failure. Cargo process output is not registry truth.

Today the roadmap still marks Reconcile as missing. Issue #102 identifies the
gap directly: ambiguous outcomes can become blind retries instead of registry
checks. Issue #99 defines the desired state machine, but Shipper still needs a
spec, ADR, plan, and implementation sequence before runtime behavior changes.

This matters because Shipper's product is trust. Without registry-truth
reconciliation, Shipper cannot honestly claim to be safer than a naive
`cargo publish` retry loop in the highest-stakes failure case.

## Users and Value

The primary users are maintainers and release operators publishing multi-crate
Rust workspaces under time pressure.

They need ambiguous publish outcomes to resolve to one of three operator-grade
answers:

- `Published`: the registry shows the version, so Shipper should mark the crate
  complete and avoid duplicate upload attempts.
- `NotPublished`: bounded registry evidence says the version is absent, so retry
  policy may continue.
- `StillUnknown`: registry truth could not be established, so Shipper must stop
  before blind retry and require operator action.

The user-facing value is a release tool that can explain what actually happened
when Cargo or the registry behaves strangely.

## Success Criteria

- Ambiguous cargo publish exits trigger registry reconciliation before retry.
- Registry evidence produces `Published`, `NotPublished`, or `StillUnknown`.
- `StillUnknown` never blind-retries.
- Reconciliation outcomes are persisted and resume honors them.
- Cargo stdout and stderr remain classification hints, not authoritative truth.
- Operators see reconciliation progress and final outcome.
- Tests cover all three outcomes and resume behavior.
- Support-tier claims are promoted only after behavior and proof exist.

## Proposed Shape

Build Reconcile through the source-of-truth stack:

```text
proposal -> spec -> ADR -> implementation plan -> active goal -> implementation PRs -> support-tier promotion
```

The durable product rule is:

```text
Cargo process output is a classification hint.
Registry state is authoritative for publish outcome.
```

The implementation lane should be split into narrow PRs:

- reconciliation types and publish events
- registry evidence collector
- ambiguous branch integration
- resume integration
- BDD and failure-mode tests
- README and support-tier promotion

## Alternatives Considered

### Keep Blind-Retry Behavior

Rejected. Blind retry is the exact behavior Shipper exists to improve. It can
turn an ambiguous upload into a duplicate publish attempt or an operator mystery.

### Trust Cargo Output Text

Rejected. Cargo stdout and stderr are useful classification hints, but they are
not authoritative for whether a registry accepted a version.

### Reconcile Only On Resume

Rejected. Resume must honor reconciliation state, but the first ambiguous
publish branch is where Shipper can prevent the unsafe retry.

### Promote README Claims Before Implementation

Rejected. Existing or future product docs must not exceed
`docs/status/SUPPORT_TIERS.md`. Ambiguous publish reconciliation remains
planned until implementation, tests, and proof artifacts exist.

## Evidence Plan

The proposal PR proves only the lane rationale and document graph:

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

The implementation lane must later prove:

- `cargo test -p shipper-types` for outcome/event types
- `cargo test -p shipper-core reconciliation` for registry evidence collection
- `cargo test -p shipper-cli --test bdd_publish` for operator behavior
- resume tests proving `Published`, `NotPublished`, and `StillUnknown` handling
- support-tier updates only after proof exists

## Risks

- Registry polling can be too aggressive unless bounded and configurable.
- A false `NotPublished` result could reintroduce blind retry risk under a new
  name.
- Treating local state as truth could make resume skip necessary reconciliation.
- Product docs may overclaim Reconcile before the support tier is promoted.
- Tests can become too coupled to real registries unless mock registry surfaces
  stay deterministic.

## Non-Goals

- Implementing publish-engine behavior in this proposal PR.
- Promoting ambiguous publish reconciliation from `planned`.
- Replacing readiness checks with reconciliation checks.
- Making registry API availability the only source of truth; sparse index
  evidence remains part of the design.
- Changing release publication behavior for 0.4.0-rc.1.

## Exit Criteria

This proposal is complete when it is followed by:

- a behavior spec for registry reconciliation
- an ADR recording registry truth over Cargo process output
- an implementation plan with PR sequencing and proof commands
- an active goal manifest pointing at the Reconcile lane

Runtime implementation begins only after those artifacts exist.
