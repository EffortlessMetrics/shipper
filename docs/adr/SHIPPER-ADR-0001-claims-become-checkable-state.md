# SHIPPER-ADR-0001: Claims Become Checkable State

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Linked ADRs:
Linked plan:
Linked issues: #109, #195
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check

## Decision

Shipper treats product claims, release readiness, and agent-executed work as
checkable state.

A stable claim requires a support-tier entry and a proof command or artifact.
An agent-executed goal requires a machine-readable target that links to the
relevant plan and spec. A policy exception requires a ledger receipt, not prose
alone.

## Context

Shipper's product is trust. The project already has policy reports,
file-policy gates, no-panic checks, Clippy policy, ripr advisory output,
mutation routing, release dry-run work, and runtime events/receipts/state.

Those signals need durable ownership. If README claims, release evidence, and
agent work live only in scattered prose, Shipper can overclaim or execute stale
plans. The source-of-truth stack exists to keep claims connected to proof.

## Consequences

- README and product claims must not outrun `docs/status/SUPPORT_TIERS.md`.
- Release-readiness claims must point to release artifacts and proof commands.
- Active goals must link to plans and specs rather than relying on issue
  archaeology.
- Specs must identify required evidence before a claim can be promoted.
- Policy exceptions belong in `policy/*.toml`; prose may explain them but cannot
  replace them.
- Advisory signals, including ripr and mutation lanes, stay advisory unless a
  later policy explicitly promotes them.

## Alternatives Considered

### README As Source Of Truth

Rejected. README text is a product surface, not the authority for claim
maturity.

### Issues As Execution Contracts

Rejected. Issues are useful tracking surfaces, but they are too drift-prone to
be the only execution target for agents.

### Prose-Only Policy Exceptions

Rejected. Exceptions need structured receipts so policy tooling and reviewers
can identify ownership, rationale, and review timing.

## Follow-Up Specs And Plans

- `docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md`
- future `plans/0.4.0/source-of-truth-stack.md`
- future release-readiness spec and plan for #195
- future registry reconciliation proposal/spec/ADR/plan before product behavior
  changes
