# SHIPPER-SPEC-0001: Source-of-Truth Stack

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs:
Linked ADRs:
Linked plan: plans/0.4.0/source-of-truth-stack.md
Linked issues: #109, #195
Linked PRs: #239, #240, #241, #242, #243, #244, #250, #251, #252, #253, #319
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: doc-contract advisory report and policy-report integration
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper's release-safety claims, operating policies, release artifacts, and
agent work need a linked source-of-truth system. Without one, claims can outrun
proof and agents can execute stale issue text instead of the current lane.

This spec defines the behavior contract for the document stack. It does not
define PR order, product rationale, or release evidence for a specific version.

## Behavior Contract

- Proposals explain why a lane exists, who benefits, alternatives considered,
  risks, and success criteria.
- Specs define behavior that must be true, non-goals, required evidence,
  acceptance examples, test mapping, and promotion rules.
- ADRs record durable architecture decisions and consequences.
- Plans define PR sequencing, proof commands, rollback, and stop conditions.
- Active goals define the current machine-readable execution target for agents.
- Support tiers define claim maturity and the proof commands or artifacts behind
  user-facing claims.
- Policy ledgers define exceptions, receipts, and enforcement state.
- Release artifacts record what happened for a specific version.
- No artifact duplicates another layer's source of truth.
- Repo-management goal state must live under `.shipper-meta/goals/`, not
  `.shipper/`.

## Non-Goals

- Implementing registry reconciliation.
- Executing #195 release proof.
- Changing runtime release behavior.
- Replacing policy ledgers with prose.
- Making doc-contract checks blocking before advisory reports exist.

## Required Evidence

For source-of-truth document changes:

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

## Acceptance Examples

- A spec that contains PR-by-PR order is incomplete; sequencing belongs in
  `plans/`.
- A README claim without a support-tier entry is incomplete once support tiers
  exist.
- A policy exception described only in prose is invalid; it belongs in
  `policy/*.toml`.
- An active goal pointing to a missing spec or plan is invalid once
  doc-contract checking exists.
- A release artifact describing future work as completed is invalid; release
  artifacts record what happened.

## Test Mapping

Proof uses existing repository gates:

- doc-contract checks for source-of-truth links and active goal references
- file-policy checks for non-Rust receipts
- policy-report checks for unified policy evidence
- format checks for repository hygiene

## Implementation Mapping

The implementation sequence belongs in `plans/0.4.0/source-of-truth-stack.md`.

The advisory checker validates:

- proposal/spec/ADR filename IDs against title IDs
- required header fields
- valid status values
- linked files when non-empty
- `.shipper-meta/goals/active.toml` TOML parsing
- active goal required top-level metadata, end-state entries, and valid
  top-level/work-item statuses
- top-level blocked-goal evidence and next-action fields
- active goal work-item `id`/`status` fields and proof-command coverage for
  `ready`, `active`, and `planned` work
- active work item references to existing specs and plans
- `docs/ci/test-evidence-lanes.md` workflow inventory coverage for every
  tracked `.github/workflows/*.yml` file, with stale inventory entries rejected
- `docs/status/SUPPORT_TIERS.md` presence, required metadata headers, valid
  status, linked proposal/spec/ADR/plan file references, and Claim Map tier
  values against the Tier Model

## CI Proof

CI runs doc-contract checks in advisory mode and uploads reports. The report
summarizes document, active-goal, workflow-inventory, and support-tier coverage
so agents can see which source-of-truth surfaces were checked. Blocking mode
should come only after the reports have burned in.

## Promotion Rule

This spec does not promote user-facing claims by itself. Claim promotion belongs
in `docs/status/SUPPORT_TIERS.md` after proof commands and artifacts exist.

## Open Questions

- Which stale-link or orphan-document checks should belong to
  `blocking-strict` mode later?
- Whether future release readiness artifacts should also emit machine-readable
  JSON.
