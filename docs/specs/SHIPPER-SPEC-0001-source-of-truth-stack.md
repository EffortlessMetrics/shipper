# SHIPPER-SPEC-0001: Source-of-Truth Stack

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Linked proposal: SHIPPER-PROP-0001
Linked ADRs: SHIPPER-ADR-0001
Linked plan: plans/0.4.0/spec-system.md
Linked issues: #109, #195
Linked PRs:
Support-tier impact: all claim-proof mapping
Policy impact: future doc-contract report

## Problem

Shipper needs docs that act as an execution contract, not just descriptive
prose. A task should be addressable from a manifest, plan, spec, proposal, ADR,
support-tier map, and policy ledger without requiring agents to infer intent
from stale issue bodies.

## Behavior Contract

1. Proposals explain why.
2. Specs define behavior and proof.
3. ADRs record durable decisions.
4. Plans define PR sequencing.
5. Active goals define current machine-readable execution.
6. Support tiers define public claim maturity.
7. Policy ledgers define exceptions and enforcement receipts.
8. No document duplicates another layer's source of truth.

## Non-Goals

- Replacing the roadmap.
- Replacing issue tracking.
- Replacing policy ledgers.
- Implementing product runtime behavior.
- Making checks blocking before advisory doc-contract reports exist.

## Required Evidence

- Source-of-truth directories and README files exist.
- Templates define required headers for future artifacts.
- `.shipper-meta/goals/active.toml` parses as TOML when introduced.
- `cargo xtask check-doc-contracts --mode advisory` writes Markdown and JSON
  reports when introduced.

## Acceptance Examples

- A spec that contains PR order is invalid; move PR order to `plans/`.
- A README claim without a support-tier entry is incomplete.
- An active goal work item pointing to a missing spec is invalid.
- A policy exception described only in prose is invalid; it belongs in
  `policy/*.toml`.
- A release-readiness artifact links proof commands instead of copying raw logs
  into every source-of-truth layer.

## Test Mapping

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`

## Implementation Mapping

- `docs/proposals/`
- `docs/specs/`
- `docs/adr/`
- `docs/status/SUPPORT_TIERS.md`
- `plans/`
- `.shipper-meta/goals/active.toml`
- `xtask/src/doc_contracts.rs`

## CI Proof

Doc-contract checking starts advisory-only and writes:

```text
target/policy/doc-contracts-report.md
target/policy/doc-contracts-report.json
```

Policy CI uploads those reports with the existing policy artifacts.

## Promotion Rule

The doc-contract checker may become blocking only after the advisory report has
run on main and the initial source-of-truth stack has no broken links.

## Open Questions

- Which stale or historical docs should be exempt from strict orphan checks?
- When should README claim scanning become part of the checker?
