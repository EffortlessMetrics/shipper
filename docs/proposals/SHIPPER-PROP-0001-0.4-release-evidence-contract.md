# SHIPPER-PROP-0001: 0.4 Release Evidence Contract

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Target milestone: 0.4.0-rc.1
Linked specs: SHIPPER-SPEC-0001, SHIPPER-SPEC-0002
Linked ADRs: SHIPPER-ADR-0001
Linked plan: plans/0.4.0/spec-system.md
Linked issues: #109, #195
Linked PRs:
Support-tier impact: release readiness proof, file-policy enforcement, no-panic baseline, ripr advisory signal
Policy impact: policy/non-rust-allowlist.toml, future doc-contract report

## Problem

Shipper's 0.4 line includes substantial release-quality work: Rust 1.95,
policy ledgers, no-panic checks, ripr advisory output, mutation routing, and
release dry-run proof. Without a source-of-truth stack, those artifacts remain
hard for users and agents to connect to product claims.

The risk is overclaiming. A README can say "safe release" while the evidence is
spread across issues, CI runs, policy reports, and local proof notes. Shipper's
product is trust, so its claims need proof paths instead of optimism.

## Users and Surfaces

- Maintainers preparing a multi-crate release.
- Operators reviewing whether a release candidate is safe to tag.
- Codex and Droid sessions executing scoped repo work.
- CI policy jobs producing machine-readable evidence.
- README and product docs that make public claims.

## Success Criteria

- Every stable release claim has a proof command or artifact.
- The 0.4.0-rc.1 readiness document is linked from this proposal or a linked
  plan.
- The active goal manifest points at the release-readiness plan before #195
  work resumes.
- Codex can identify next work from `.shipper-meta/goals/active.toml` without
  scraping issue text.
- Support tiers distinguish stable, advisory, experimental, and planned claims.

## Proposed Shape

Add a source-of-truth stack:

```text
proposal -> spec -> ADR -> plan -> active goal -> proof command -> artifact
```

Each layer has one job:

- Proposals explain why.
- Specs define behavior and proof.
- ADRs record durable decisions.
- Plans define PR sequencing and proof commands.
- Active goals define current machine-readable execution state.
- Support tiers map product claims to proof.
- Policy ledgers encode receipts, exceptions, and enforcement state.

## Alternatives Considered

### Keep Using Issues as the Plan

Issues are useful for tracking, but they drift. Long issue bodies are not a
stable execution contract for agents or CI.

### Put Goal State Under `.shipper/`

Rejected. `.shipper/` is Shipper runtime state and artifact space. Repository
goal metadata belongs under `.shipper-meta/`.

### Let README Claims Lead

Rejected. README claims should be downstream of support tiers and proof
artifacts, not the authority for what is stable.

## Specs to Create or Update

- `docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md`
- `docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md`
- Future registry reconciliation spec after the 0.4 release proof lane.

## Architecture Decisions Needed

- `docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md`

## Implementation Campaign Shape

1. Add source-of-truth scaffolding.
2. Add templates.
3. Add this proposal.
4. Add source-of-truth and release-readiness specs.
5. Add support tiers and claims-as-checkable-state ADR.
6. Add an active goal manifest and plan.
7. Add advisory doc-contract checking.
8. Use the stack for #195 release readiness proof.

## Evidence Plan

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo xtask check-doc-contracts --mode advisory` once implemented
- `docs/release/0.4.0-readiness.md` for the first release evidence packet

## Risks

- The stack becomes prose-only and does not constrain execution.
- Documents duplicate each other and make ownership unclear.
- Agents infer missing links instead of fixing them.
- Support tiers lag behind README claims.

## Non-Goals

- Implementing registry reconciliation in this lane.
- Replacing policy ledgers with prose docs.
- Moving Shipper runtime state out of `.shipper/`.
- Making doc-contract checks blocking before advisory reports exist.

## Exit Criteria

- The scaffold, templates, support tiers, ADR, plan, and active goal exist.
- `cargo xtask check-doc-contracts --mode advisory` reports the document graph.
- Policy report includes doc-contract output.
- #195 release readiness proof is implemented through the stack.
