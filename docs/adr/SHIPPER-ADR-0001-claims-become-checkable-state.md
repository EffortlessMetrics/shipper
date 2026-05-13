# SHIPPER-ADR-0001: Claims Become Checkable State

Status: accepted
Date: 2026-05-13
Owner: EffortlessMetrics
Linked proposal: SHIPPER-PROP-0001
Linked specs: SHIPPER-SPEC-0001, SHIPPER-SPEC-0002
Linked plan: plans/0.4.0/spec-system.md

## Decision

Shipper treats public product claims, release readiness, and agent-executed
goals as checkable state.

A claim is not stable unless it has a proof command or artifact. A goal is not
actionable unless it points to a plan and spec. A policy exception is not valid
unless it is receipted in a policy ledger.

## Context

Shipper's product is trust. The repo already has policy ledgers, no-panic
baselines, ripr advisory output, mutation routing, and release dry-run proof.
Those artifacts need a document graph that tells users and agents what each
artifact proves and which claims it supports.

## Consequences

- README claims must map to support tiers.
- Release claims must map to readiness artifacts.
- Agent goals must map to specs and plans.
- Specs must identify required evidence.
- Policy exceptions belong in ledgers, not prose.
- CI can report document-contract drift before it becomes a release claim.

## Alternatives Considered

### Keep Claims in README Only

Rejected. README text is useful for users, but it is not precise enough to be
the authority for claim maturity.

### Use Issues as the Contract

Rejected. Issues are useful trackers, but they are mutable conversation spaces
and can drift from current code.

### Make Policy Ledgers Carry Rationale

Rejected. Ledgers receipt exceptions and enforcement state. They should not
also carry product rationale or PR sequencing.

## Follow-up Specs / Plans

- `docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md`
- `docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md`
- `plans/0.4.0/spec-system.md`
- Future registry reconciliation proposal/spec/ADR/plan
