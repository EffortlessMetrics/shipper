# Plan: Registry Reconciliation

Status: implemented
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: post-0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0002-registry-truth-and-reconciliation.md
Linked specs: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Linked ADRs: docs/adr/SHIPPER-ADR-0002-registry-truth-over-cargo-output.md
Linked plan:
Linked issues: #99, #102, #109
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## End State

Shipper reconciles ambiguous `cargo publish` outcomes against registry truth
before retrying or resuming. Ambiguous outcomes resolve to `Published`,
`NotPublished`, or `StillUnknown`, with structured evidence in events and state.

The support-tier claim remained `planned` until implementation and tests proved
all required outcomes and resume behavior. The current claim tier is maintained
in `docs/status/SUPPORT_TIERS.md`.

## PR Sequence

### PR 1 - Reconciliation types and events

Linked spec: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Blocks: PR 2
Blocked by:

#### Goal

Add reconciliation outcome/evidence types and publish events without changing
publish behavior.

#### Production Delta

New types and event variants only.

Likely ownership:

- `crates/shipper-types`
- event/state tests that compile against the new types

#### Non-Goals

Registry querying, ambiguous branch behavior, resume behavior, CLI narration,
and support-tier promotion.

#### Acceptance

- Reconciliation outcomes include `Published`, `NotPublished`, and
  `StillUnknown`.
- Evidence can record source, attempts, timestamps, and final observation.
- Event types exist for reconciliation start and result.
- Existing state/event serialization remains compatible or has an explicit
  migration path.

#### Proof Commands

- `cargo test -p shipper-types`
- `cargo test -p shipper-core state`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the new types and events if the state model changes before behavior
integration.

### PR 2 - Registry evidence collector

Linked spec: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Blocks: PR 3
Blocked by: PR 1

#### Goal

Collect bounded registry evidence for a package/version after an ambiguous
publish result.

#### Production Delta

Reusable evidence collector behind tests, not yet wired into publish retry.

Likely ownership:

- `crates/shipper-core` registry/readiness integration
- mock registry tests

#### Non-Goals

Changing retry behavior, resume behavior, CLI narration, and support-tier
promotion.

#### Acceptance

- Sparse-index and registry API paths can contribute evidence.
- Collector distinguishes found, absent, and inconclusive evidence.
- Polling is bounded and configurable.
- Tests use mock registry surfaces, not real registries.

#### Proof Commands

- `cargo test -p shipper-core registry`
- `cargo test -p shipper-core reconciliation`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Remove the collector if the evidence boundary needs redesign before wiring.

### PR 3 - Ambiguous publish branch integration

Linked spec: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Blocks: PR 4
Blocked by: PR 2

#### Goal

Replace blind retry on ambiguous cargo publish exits with registry
reconciliation.

#### Production Delta

Ambiguous publish branch behavior changes:

- emit `publish_reconciling`
- collect registry evidence
- apply `Published`, `NotPublished`, or `StillUnknown`
- never blind-retry `StillUnknown`

#### Non-Goals

Resume integration, CLI narration polish, support-tier promotion, and unrelated
retry-policy changes.

#### Acceptance

- `Published` marks the package complete.
- `NotPublished` allows retry policy to continue.
- `StillUnknown` stops with operator-visible state.
- Cargo output remains a classification hint only.

#### Proof Commands

- `cargo test -p shipper-core reconciliation`
- `cargo test -p shipper-core publish`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the ambiguous branch integration while keeping the collector and types if
they remain useful behind tests.

### PR 4 - Resume honors reconciliation state

Linked spec: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Blocks: PR 5
Blocked by: PR 3

#### Goal

Make resume honor persisted reconciliation outcomes.

#### Production Delta

Resume behavior changes:

- `Published` skips upload
- `NotPublished` is retry-eligible
- `StillUnknown` requires explicit operator action

#### Non-Goals

New operator command design unless required to stop safely, CLI narration polish,
and support-tier promotion.

#### Acceptance

- Resume cannot blind-retry a persisted `StillUnknown`.
- Resume skips reconciled `Published`.
- Resume remains idempotent across state reloads.

#### Proof Commands

- `cargo test -p shipper-core resume`
- `cargo test -p shipper-core state`
- `cargo test -p shipper-cli --test bdd_publish`
- `cargo xtask policy-report`

#### Rollback

Revert resume integration if persisted state semantics need redesign.

### PR 5 - BDD and failure-mode tests

Linked spec: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Blocks: PR 6
Blocked by: PR 4

#### Goal

Prove Reconcile behavior through operator-facing and failure-mode tests.

#### Production Delta

No new behavior beyond test hardening unless gaps are found.

#### Non-Goals

README/support-tier promotion and unrelated BDD cleanup.

#### Acceptance

- BDD covers ambiguous publish resolving to `Published`.
- BDD covers ambiguous publish resolving to `NotPublished`.
- BDD covers ambiguous publish resolving to `StillUnknown`.
- BDD or integration coverage proves resume behavior.
- Failure-mode docs align with observed behavior.

#### Proof Commands

- `cargo test -p shipper-cli --test bdd_publish`
- `cargo test --workspace --all-features`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the tests if behavior is split differently, but keep any bug fixes that
the tests exposed in their own PRs.

### PR 6 - Claim promotion and product docs

Linked spec: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Blocks:
Blocked by: PR 5

#### Goal

Promote Reconcile claims only after implementation and proof exist.

#### Production Delta

Documentation and support-tier updates only.

#### Non-Goals

Runtime behavior changes.

#### Acceptance

- `docs/status/SUPPORT_TIERS.md` promotes ambiguous publish reconciliation only
  to the tier proven by implementation.
- README and product docs do not exceed the support tier.
- Release/readiness docs link the reconciliation evidence instead of duplicating
  it.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert claim promotion if implementation proof is incomplete or support-tier
wording overclaims.
