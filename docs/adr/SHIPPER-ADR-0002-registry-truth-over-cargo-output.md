# SHIPPER-ADR-0002: Registry Truth Over Cargo Output

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: post-0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0002-registry-truth-and-reconciliation.md
Linked specs: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan:
Linked issues: #99, #102, #109
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## Decision

For ambiguous publish outcomes, Shipper treats Cargo stdout, stderr, and exit
code as classification hints. Registry state is authoritative for publish
outcome.

The engine must not blind-retry an ambiguous `cargo publish` result. It must
first reconcile against bounded registry evidence and resolve to `Published`,
`NotPublished`, or `StillUnknown`.

## Context

Cargo can return an error after an upload may already have reached the registry.
In that case, the process result alone cannot tell the operator whether retrying
is safe. Shipper's value is release control, not just command execution, so this
ambiguity must be closed against registry truth.

The roadmap marked Reconcile as the largest missing safety gap. The source of
truth for behavior is `SHIPPER-SPEC-0003`; this ADR records the durable
architecture decision behind that behavior.

## Consequences

- Ambiguous publish handling must query sparse-index and/or registry API
  evidence before retrying.
- `Published` reconciliation marks the package complete and prevents re-upload.
- `NotPublished` reconciliation allows retry policy to continue.
- `StillUnknown` reconciliation stops for operator action.
- Resume must honor persisted reconciliation outcomes.
- Event logs must expose reconciliation start and result.
- Cargo output parsing can remain useful for classification, but it cannot be
  the final safety-critical answer.
- Product docs and README claims must not exceed the Reconcile support tier in
  `docs/status/SUPPORT_TIERS.md`.

## Alternatives Considered

### Trust Cargo Exit Status

Rejected. Exit status cannot distinguish "upload failed" from "upload succeeded
but the post-upload process failed" in ambiguous cases.

### Parse Human-Readable Cargo Output As Truth

Rejected. Cargo output is not a stable registry-state API. It can classify
failure shape but cannot prove publication outcome.

### Retry First, Reconcile Later

Rejected. Retrying before registry evidence can create duplicate-upload pressure
and obscures the actual outcome.

### Treat Local State As Truth On Resume

Rejected. Local state is a projection. When publish outcome is ambiguous,
registry reconciliation must either produce truth or leave an explicit
`StillUnknown` operator stop.

## Follow-Up Specs And Plans

- `docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md`
- future `plans/reconcile/implementation-plan.md`
- future `.shipper-meta/goals/active.toml` update for Reconcile execution
