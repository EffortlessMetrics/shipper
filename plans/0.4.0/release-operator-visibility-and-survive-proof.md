# Plan: Release Operator Visibility and Survive Proof

Status: implemented
Owner: EffortlessMetrics
Created: 2026-05-18
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md; docs/adr/SHIPPER-ADR-0002-registry-truth-over-cargo-output.md
Linked plan: plans/0.4.0/json-evidence-contracts.md
Linked issues: #109
Linked PRs: #330; #331; #333; #310; #335; #336
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: no new policy exceptions
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## End State

Shipper release execution is visible and survivable:

- event-follow consumers do not misread incomplete JSONL tail lines
- finalization detects material event/state/receipt/reconciliation drift
- state can be rebuilt from events for recovery or comparison
- live-runner interruption is proven by an uploaded `.shipper/` evidence packet
  before the support-tier claim is promoted

Existing foundations, including attempt history, wait/readiness events, status
watch JSON, and synthetic resume proof, are treated as already landed baseline.

## PR Sequence

### PR 1 - Source-of-truth activation

Linked spec: docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Blocks: PR 2
Blocked by:

#### Goal

Define the behavior contract, implementation plan, active goal, and planned
support-tier hooks for release operator visibility and survive proof.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Runtime behavior, schema changes, drift checks, state rebuild, live-runner
rehearsal, and support-tier promotion.

#### Acceptance

- Spec and plan exist and link to each other.
- `.shipper-meta/goals/active.toml` points to this lane.
- Support tiers remain planned/advisory and do not promote unproven claims.

#### Proof Commands

- `python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/active.toml').read_text()); print('active goal TOML parses')"`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Revert this spec, plan, active goal, and support-tier planned rows if the lane
is superseded before runtime work depends on it.

### PR 2 - Inspect-events follow incomplete-tail hardening

Linked spec: docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Blocks: PR 3
Blocked by: PR 1

#### Goal

Make `inspect-events --follow` stream only complete JSONL entries and retry an
incomplete final line on the next poll.

#### Production Delta

CLI event-log reading and rendering only.

#### Non-Goals

Event schema changes, status/watch output changes, state rebuild, drift checks,
and live-runner rehearsal.

#### Acceptance

- Human and JSON follow modes emit no partial event for an incomplete tail line.
- The offset remains before an incomplete final line.
- When the line becomes complete, it emits exactly once.
- Completed malformed entries remain visible as actionable errors.

#### Proof Commands

- `cargo test -p shipper-cli inspect_events --lib --locked`
- `cargo test -p shipper-cli --test cli_e2e --locked`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Restore prior inspect-events behavior if follow mode creates misleading output.

### PR 3 - Events-as-truth drift checks

Linked spec: docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Blocks: PR 4
Blocked by: PR 2

#### Goal

Verify event/state/receipt/reconciliation consistency at publish finalization.

#### Production Delta

Finalization validation only.

#### Non-Goals

State rebuild and live interruption rehearsal.

#### Acceptance

- Finalization detects package state without matching events. [Landed in #333]
- Reconciliation evidence is required when ambiguity occurred. [Landed in #333]
- Receipt summaries match final state or fail with actionable drift evidence. [Landed in #333]

#### Proof Commands

- `cargo test -p shipper-core drift --lib --locked`
- `cargo test -p shipper-core state --lib --locked`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Revert drift checks if they block valid releases or misclassify evidence.

### PR 4 - Rebuild state from events

Linked spec: docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Blocks: PR 5
Blocked by: PR 3

#### Goal

Build a recoverable state projection from `events.jsonl`.

#### Production Delta

Library recovery function first; CLI command only if it remains narrow.

#### Non-Goals

Changing normal resume behavior or live interruption rehearsal.

#### Acceptance

- Events can reconstruct published, pending, failed, ambiguous, and reconciled
  package states. [Landed in #310]
- Rebuilt state can be compared with current state for drift diagnostics.

#### Proof Commands

- `cargo test -p shipper-core rebuild --lib --locked`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Remove rebuild support if reconstruction is incomplete or misleading.

### PR 5 - Live interruption rehearsal

Linked spec: docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Blocks: PR 6
Blocked by: PR 4

#### Goal

Prove that a real runner interruption can upload `.shipper/`, resume from
artifacts, avoid duplicate publishes, and leave coherent evidence.

#### Production Delta

CI/release rehearsal workflow and evidence artifacts only.

#### Non-Goals

Publishing a production release or promoting the support-tier claim before the
artifact exists.

#### Acceptance

- Controlled interruption produces a `.shipper/` artifact.
- Resume uses the artifact and completes without duplicate publish.
- Events, state, receipt, and reconciliation evidence remain coherent.
- GitHub Actions run 26051581056 uploaded
  `shipper-live-interruption-seed-26051581056` and
  `shipper-live-interruption-resume-26051581056`. [Landed in #335]

#### Proof Commands

- dedicated interruption rehearsal workflow
- `cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`
- `cargo xtask policy-report`
- `git diff --check`

#### Rollback

Disable the rehearsal workflow and revert support-tier text if proof is
unreliable.

### PR 6 - Support-tier promotion

Linked spec: docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Blocks:
Blocked by: PR 5

#### Goal

Promote only the claims proven by the implementation and rehearsal artifacts.

#### Production Delta

Documentation/status only.

#### Non-Goals

Runtime changes.

#### Acceptance

- Support tiers name exact proof commands and artifacts.
- README and product claims do not exceed proven evidence.
- `docs/status/SUPPORT_TIERS.md` names the main-run proof artifacts and keeps
  the claim scoped to real runner artifact recovery with fake Cargo/mock
  registry proof.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Demote any claim whose proof artifact is missing or weak.
