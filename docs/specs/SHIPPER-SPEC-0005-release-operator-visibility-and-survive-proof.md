# SHIPPER-SPEC-0005: Release Operator Visibility and Survive Proof

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-18
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md; docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md; docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md; docs/adr/SHIPPER-ADR-0002-registry-truth-over-cargo-output.md
Linked plan: plans/0.4.0/release-operator-visibility-and-survive-proof.md
Linked issues: #109
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper already has several visibility foundations: state carries attempt
history, publish/readiness events record retry and wait schedules, status-watch
JSON is versioned, and synthetic resume proof exists. The remaining product gap
is to turn those pieces into a durable release black-box recorder that survives
real interruption and lets operators or agents follow event streams without
misreading partial evidence.

The immediate gap is narrow but important: event-follow consumers must not
break or emit misleading output when a writer has flushed only part of the final
JSONL line. The larger survive gap is that state, events, receipts, and
reconciliation evidence must be checked and rebuildable before live-runner
interruption can be promoted from planned to stable.

## Behavior Contract

Existing stable and internal foundations remain part of this lane's baseline:

- execution state records attempt history for publish attempts
- retry and readiness scheduling events exist as durable event variants
- `shipper status --watch --format json` emits `shipper.status.watch.v1`
- synthetic interruption/resume proof exists against fake Cargo and mock
  registry surfaces

Future changes in this lane must preserve and harden these rules:

- `inspect-events --follow` must stream only complete events
- if the final JSONL line is incomplete, follow mode must keep the read offset
  before that line and retry on the next poll
- text and JSON follow modes must not treat partial JSON as a valid event
- malformed completed entries must produce actionable output instead of hiding
  evidence corruption
- event follow output must not contain unredacted tokens or sensitive command
  output
- `events.jsonl` remains the authoritative release timeline
- `state.json` remains the resumable projection
- `receipt.json` remains the final summary
- `reconciliation.json`, when present, remains the ambiguity evidence
  projection
- finalization must detect material drift between events, state, receipt, and
  reconciliation evidence
- rebuild-from-events must produce a state projection for recovery or
  comparison
- live-runner interruption claims require a real rehearsal artifact before
  support-tier promotion

## Non-Goals

- Re-implementing attempt history, retry events, readiness events, or
  status-watch JSON that already exist.
- Changing publish order or dependency planning.
- Changing registry reconciliation outcomes.
- Replacing existing command-owned JSON envelopes.
- Making live-runner interruption stable before a real runner rehearsal exists.
- Publishing crates, tagging a release, or creating release artifacts.
- Adding receipt-driven remediation behavior in this spec.

## Required Evidence

Source-of-truth proof for this spec:

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

Future implementation proof must cover:

- incomplete tail handling in `inspect-events --follow`
- completed malformed event handling
- finalization drift checks between events, state, receipt, and reconciliation
  evidence
- rebuild-from-events producing a comparable execution-state projection
- live-runner interruption rehearsal that uploads `.shipper/`, resumes from
  artifacts, and proves no duplicate publish

## Acceptance Examples

- A writer appends half of an event line. `inspect-events --follow` emits
  nothing for that partial line and retries it on the next poll.
- The same partial line later receives its trailing newline. Follow mode emits
  the event exactly once.
- A completed malformed JSONL entry is reported as evidence corruption; it is
  not silently skipped as if release progress were healthy.
- Finalization sees a package in `state.json` with no corresponding durable
  event. It reports drift instead of producing a misleading receipt.
- Rebuild-from-events reconstructs package states from the event stream and can
  be compared with `state.json`.
- A live interruption claim remains planned if the proof is only synthetic or
  local; promotion requires a real runner rehearsal artifact.

## Test Mapping

Expected implementation proof:

- `cargo test -p shipper-cli inspect_events --lib --locked`
- `cargo test -p shipper-cli --test cli_e2e --locked`
- `cargo test -p shipper-core drift --lib --locked`
- `cargo test -p shipper-core state_rebuild --lib --locked`
- `cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`

Tests must use fake Cargo and mock registries. Real registry proof belongs only
in the live-runner rehearsal artifact and release-readiness evidence.

## Implementation Mapping

The implementation plan belongs in
`plans/0.4.0/release-operator-visibility-and-survive-proof.md`.

The lane should land in narrow PRs:

- source-of-truth activation
- inspect-events follow incomplete-tail hardening
- finalization drift checks
- rebuild state from events
- live interruption rehearsal
- support-tier promotion only after proof exists

## CI Proof

CI should prove unit, integration, BDD, policy, and doc-contract gates for each
implementation PR. Live interruption proof should run in a dedicated workflow
or release rehearsal job that uploads the `.shipper/` evidence packet.

CI success is not sufficient for support-tier promotion unless the relevant
artifact proves the specific claim.

## Promotion Rule

Support-tier claims may move only when the named proof exists:

- inspect-events follow hardening can be stable after focused tests prove
  incomplete-tail and malformed-entry behavior
- events/state/receipt drift detection stays planned until finalization checks
  fail on inconsistent evidence
- rebuild-from-events stays planned until tests prove a reconstructed projection
- live-runner interruption stays planned until a real runner rehearsal artifact
  proves artifact recovery and safe resume

## Open Questions

- Should rebuild-from-events be a library-only capability first or a CLI command
  in the same lane?
- Which live-runner rehearsal should be safe enough before crates.io dogfood
  release proof?
