# SHIPPER-PROP-0001: Source-of-Truth and Release Evidence

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal:
Linked specs:
Linked ADRs:
Linked plan:
Linked issues: #109, #195
Linked PRs:
Support-tier impact: future support-tier claim map
Policy impact: policy ledgers remain the source of truth for exceptions and receipts
Proof commands: cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper already has strong proof pieces: policy reports, file-policy gates,
no-panic checks, Clippy policy, ripr advisory output, mutation routing, release
dry-run work, and runtime events/receipts/state. Those pieces are not yet tied
into a repo-native claim system.

That makes drift easy. A user-facing claim can live in README prose while its
proof lives in CI output, issue comments, local release notes, or chat history.
An agent can also pick up a stale issue and execute the wrong lane. For a
release tool whose product is trust, that is the wrong failure mode.

## Users and Value

The primary users are maintainers preparing multi-crate Rust releases,
operators reviewing release readiness, and agents executing scoped repo work.

They need to trace a claim or task to:

- why it exists
- what behavior is promised
- which decision made it durable
- which plan sequences the work
- which command proves it
- which artifact records it
- which support tier the user can rely on

## Success Criteria

- Codex can start from `.shipper-meta/goals/active.toml`, follow links to a plan
  and spec, and run named proof commands without scraping issue prose.
- README and product claims map to support tiers before they are promoted as
  stable.
- #195 can be executed from a release-readiness spec and implementation plan.
- Registry reconciliation can be prepared through proposal, spec, ADR, and plan
  before product behavior changes.
- Policy exceptions remain in `policy/*.toml`; prose can explain them but cannot
  replace the receipt.

## Proposed Shape

Install a linked source-of-truth stack:

```text
proposal -> spec -> ADR -> plan -> active goal -> proof command -> artifact
```

Each layer has one job:

- proposals explain why
- specs define behavior and required evidence
- ADRs record durable architecture decisions
- plans define PR sequencing, rollback, and proof commands
- active goals define current machine-readable execution state
- support tiers map claims to proof commands and artifacts
- policy ledgers receipt exceptions and enforcement state
- release artifacts record what happened for a specific version

The Shipper-specific namespace rule is part of the proposal: repo-management
goal state belongs under `.shipper-meta/goals/`, never under `.shipper/`.
`.shipper/` remains Shipper runtime state and artifact space.

## Alternatives Considered

### Keep Using Issues as the Plan

Issues are useful tracking surfaces, but they drift. Long issue bodies are not a
stable execution contract for CI or agents.

### Put Goal State Under `.shipper/`

Rejected. `.shipper/` is product runtime state. Mixing repo-management goals
with runtime publish state would make both surfaces less trustworthy.

### Let README Claims Lead

Rejected. README claims should be downstream of support tiers and proof
artifacts, not the authority for what is stable.

## Evidence Plan

Initial proof is repository-local:

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

Later proof adds `cargo xtask check-doc-contracts --mode advisory`, then policy
report integration, CI advisory reporting, and eventually blocking mode after
the advisory reports have burned in.

## Risks

- The stack becomes decorative prose instead of constraining execution.
- Documents duplicate each other and make ownership unclear.
- Agents infer missing links instead of fixing them in a separate PR.
- Support tiers lag behind README claims.
- Release artifacts describe future intent instead of recording what happened.

## Non-Goals

- Implementing registry reconciliation in this proposal.
- Executing #195 before the release-readiness spec and plan exist.
- Replacing policy ledgers with prose docs.
- Adding doc-contract checker code in the proposal PR.
- Moving Shipper runtime state out of `.shipper/`.

## Exit Criteria

The lane is successful when:

- scaffold and templates exist
- this proposal has linked specs, ADRs, plans, active goals, and support tiers
- doc-contract checking exists in advisory mode
- policy report includes doc-contract status
- CI runs doc-contract checks in advisory mode
- #195 release readiness proof is executed through the stack
- Reconcile has proposal, spec, ADR, and implementation plan before behavior
  changes begin
