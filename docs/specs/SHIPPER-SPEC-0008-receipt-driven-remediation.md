# SHIPPER-SPEC-0008: Receipt-Driven Remediation

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-19
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md; docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/receipt-driven-remediation.md
Linked issues: #98; #104; #109
Linked PRs: #344; #345
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for workflow, process, network, and file receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper's release-closure promise does not end when crates are published. A
bad partial release or later-discovered compromised crate leaves operators with
manual containment work: identify affected crate versions, choose yank order,
decide whether to fix-forward, and preserve an audit trail.

Current code and docs already contain several Remediate foundations: `shipper
yank`, `shipper plan-yank`, `shipper fix-forward`, compromised receipt fields,
`PackageYanked` events, a cargo-yank wrapper, and a how-to guide. The remaining
source-of-truth gap is that these behaviors are not yet tied into a spec,
support-tier claim, active goal, and proof sequence.

## Behavior Contract

Receipt-driven remediation must preserve these rules:

- Remediation starts from a prior `receipt.json`; Shipper must not infer a bad
  release from chat, issue text, or stdout alone.
- Yanking is containment, not undo. It prevents new dependency resolves but does
  not remove already-downloaded bytes or existing lockfile pins.
- A remediation plan must identify the source receipt, target registry, target
  crate versions, operator reason, affected packages, and command sequence.
- Yank planning must be deterministic and reverse topological: dependents
  before dependencies.
- Fix-forward planning must be deterministic and publish-directional:
  dependencies before dependents.
- Compromise markers in receipts must be explicit operator annotations.
- Execution that invokes `cargo yank` must record event evidence and avoid
  logging token values.
- JSON output, when present, must use a schema-versioned command or plan
  envelope before it is promoted as a stable integration surface.
- Support-tier claims must distinguish implemented primitives, planning-only
  remediation, guarded execution, and full mechanical recovery.

## Non-Goals

- Claiming yanking deletes crates.io versions or invalidates existing lockfiles.
- Editing Cargo.toml versions automatically.
- Publishing fix-forward successors automatically.
- Adding crates.io team management.
- Replacing registry reconciliation or publish ordering behavior.
- Promoting remediation as stable before proof commands and artifacts are named.

## Required Evidence

Source-of-truth proof for this spec:

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

Future implementation and promotion proof must cover:

- CLI help and snapshots for `yank`, `plan-yank`, and `fix-forward`
- `cargo yank` wrapper tests that prove command construction, registry
  selection, exit-code handling, and token redaction
- plan-yank tests for all-published, compromised-only, and starting-crate graph
  modes
- fix-forward tests for compromised receipts and empty-compromise receipts
- event evidence for `PackageYanked`
- JSON contract proof before declaring remediation plan JSON stable
- support-tier rows that name exact tests, commands, and artifacts

## Current Proof Map

The existing implementation is intentionally split into bounded primitives.
These are proven today:

- `cargo test -p shipper-core plan_yank --lib --locked` proves the
  reverse-topological yank planner, all-published mode, compromised-only mode,
  starting-crate graph mode, explicit reasons, and yank-plan JSON roundtrip.
- `cargo test -p shipper-core fix_forward --lib --locked` proves the
  fix-forward planner for compromised published receipt entries, empty
  compromise receipts, topological successor ordering, and human text output.
- `cargo test -p shipper-core cargo_yank --lib --locked` proves the
  `cargo yank` wrapper command construction, registry flag handling for
  crates.io and custom registries, output capture, and nonzero exit handling.
- `cargo test -p shipper-types package_receipt_roundtrip --lib --locked` and
  `cargo test -p shipper-types receipt_roundtrip --lib --locked` prove receipt
  serialization keeps the remediation marker fields in the durable receipt
  shape.
- `cargo test -p shipper-core event_types_serialize_correctly --lib --locked`
  proves the event enum serializes as part of the existing event-type surface.
- `cargo test -p shipper-cli --test e2e_expanded --locked
  help_yank_snapshot`, `cargo test -p shipper-cli --test e2e_expanded
  --locked help_plan_yank_snapshot`, and `cargo test -p shipper-cli --test
  e2e_expanded --locked help_fix_forward_snapshot` prove the CLI command
  contracts are visible in help output.

These are not proven by this map and must not be promoted yet:

- end-to-end CLI execution of `shipper yank`, `shipper yank --plan`,
  `shipper plan-yank`, or `shipper fix-forward`
- targeted `PackageYanked` event serialization or event-log tests
- stable command-owned JSON envelopes for remediation plans
- `.shipper/remediation-plan.json` artifact emission
- guarded live yank execution beyond fake Cargo/unit-level proof

## Acceptance Examples

- Given a receipt with packages published in topological order `core`, `api`,
  `cli`, a yank plan for all published packages emits `cli`, `api`, `core`.
- Given the same receipt and a compromised marker only on `api`, a
  compromised-only yank plan emits `api`.
- Given a starting crate `core`, graph mode includes the crates that
  transitively depend on `core`, ordered dependents first.
- Given a compromised receipt, fix-forward output tells the operator which
  successor versions to create without editing manifests or publishing.
- Given a direct `shipper yank --mark-compromised`, the matching receipt entry
  gains compromise metadata and the event log records `PackageYanked`.

## Test Mapping

Expected proof:

- `cargo test -p shipper-core plan_yank --lib --locked`
- `cargo test -p shipper-core fix_forward --lib --locked`
- `cargo test -p shipper-core cargo_yank --lib --locked`
- `cargo test -p shipper-types package_receipt_roundtrip --lib --locked`
- `cargo test -p shipper-types receipt_roundtrip --lib --locked`
- `cargo test -p shipper-core event_types_serialize_correctly --lib --locked`
- `cargo test -p shipper-cli --test e2e_expanded --locked help_yank_snapshot`
- `cargo test -p shipper-cli --test e2e_expanded --locked help_plan_yank_snapshot`
- `cargo test -p shipper-cli --test e2e_expanded --locked help_fix_forward_snapshot`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`

## Implementation Mapping

The implementation plan belongs in
`plans/0.4.0/receipt-driven-remediation.md`.

The lane should land in narrow PRs:

- source-of-truth activation
- proof mapping and support-tier baseline for existing remediation surfaces
- remediation plan JSON contract if the current JSON shape is stable enough
- evidence artifact wiring for `.shipper/remediation-plan.json`
- guarded execution hardening only after planning proof is stable

## CI Proof

CI should prove unit, integration, policy, and doc-contract gates for each PR.
Remediation execution tests must use fake Cargo and mock registries; live
crates.io yanks are release-operator actions and must not run in PR CI.

## Promotion Rule

Support-tier claims may move only when the named proof exists:

- `shipper yank` can be stable only for the bounded containment primitive if
  command, event, receipt, and redaction proof exists.
- `shipper plan-yank` can be stable only after deterministic plan tests and
  docs prove the yank-order contract.
- `shipper fix-forward` stays advisory until it has a documented JSON contract
  or explicitly remains a human-readable planning aid.
- Full mechanical remediation stays planned until dry-run artifacts and guarded
  execution proof exist.

## Open Questions

- Should `.shipper/remediation-plan.json` be produced by `plan-yank`,
  `fix-forward`, or a future `remediate --dry-run` command?
- Should receipt history lookup be a separate command before guarded yank
  execution is promoted?
