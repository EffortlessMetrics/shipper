# SHIPPER-PROP-0001: Source-of-Truth and Release Evidence

Status: implemented
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal:
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md; docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/source-of-truth-stack.md; plans/0.4.0/release-readiness-proof.md
Linked issues: #109, #195
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain the source of truth for exceptions and receipts
Proof commands: cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper had strong proof pieces—policy reports, file-policy gates, no-panic
checks, Clippy policy, advisory analysis, mutation routing, release dry-run work,
and runtime events/receipts/state—but those pieces were not tied into a
repo-native claim system.

That made drift easy. A user-facing claim could live in README prose while its
proof lived in CI output, issue comments, local release notes, or chat history.
An agent could also pick up a stale issue and execute the wrong lane. For a
release tool whose product is trust, that was the wrong failure mode.

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

- Agents can follow linked plans and specs and run named proof commands without
  reconstructing the contract from issue prose.
- README and product claims map to support tiers before they are promoted as
  stable.
- #195 was executed from a release-readiness spec and implementation plan.
- Registry reconciliation was prepared through proposal, spec, ADR, and plan
  before product behavior changed.
- Policy exceptions remain in `policy/*.toml`; prose can explain them but cannot
  replace the receipt.

## Implemented Shape

The repository uses a linked source-of-truth stack:

```text
proposal -> spec -> ADR -> plan -> current execution state -> proof command -> artifact
```

Each layer has one job:

- proposals explain why
- specs define behavior and required evidence
- ADRs record durable architecture decisions
- plans define PR sequencing, rollback, and proof commands
- current execution records identify the bounded work now in flight
- support tiers map claims to proof commands and artifacts
- policy ledgers receipt exceptions and enforcement state
- release artifacts record what happened for a specific version

The Shipper-specific namespace rule remains part of the proposal:
repo-management control state must not be written under `.shipper/`.
`.shipper/` remains Shipper runtime state and artifact space. The repository no
longer treats a global active-goal pointer as required authority; current work
is selected from the reviewed issue/PR/release program and its linked evidence.

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

## Evidence Operation

Repository-local proof includes:

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `cargo xtask check-doc-contracts --mode advisory`

Release proof is exact-source evidence rather than a generic green status. The
0.4.0 implementation record is `docs/release/0.4.0-readiness.md`; the current
0.5.0 authority chain is tracked through the release preparation and
promotion/rehearsal issues. Policy reports and doc checks can expose drift, but
they do not authorize tags, publication, or public claim promotion by
themselves.

## Risks

- The stack becomes decorative prose instead of constraining execution.
- Documents duplicate each other and make ownership unclear.
- Agents infer missing links instead of fixing them in a separate PR.
- Support tiers lag behind README claims.
- Release artifacts describe future intent instead of recording what happened.
- Historical plans are mistaken for current execution instructions.

## Non-Goals

- Replacing policy ledgers with prose docs.
- Moving Shipper runtime state out of `.shipper/`.
- Treating a linked document graph as publication authorization.
- Requiring one global repository scheduler or active-goal pointer.
- Rewriting historical evidence to match later implementation.

## Completion Evidence

The implemented repository contains the proposal/spec/ADR/plan stack, support
tiers, policy and document checks, release evidence artifacts, and the 0.4.0
readiness record produced through that stack. Registry reconciliation has its
own implemented proposal, spec, ADR, plan, tests, and support-tier mapping.

The remaining maintenance obligation is to keep links, statuses, claims, and
proof commands current. That obligation is ongoing repository hygiene; it does
not return this proposal to `proposed`.
