# SHIPPER-SPEC-0004: JSON Evidence Contracts

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-18
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/json-evidence-contracts.md
Linked issues: #109
Linked PRs: #315, #316, #317, #318
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain the source of truth for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo xtask package-surface; cargo fmt --all -- --check

## Problem

Shipper is becoming release-closure infrastructure. Humans can read the CLI
output, but CI systems, internal developer portals, and agents need stable JSON
evidence that is boring to parse and hard to misread.

Recent work added versioned JSON identifiers to more command surfaces, but the
repo still needs one behavior contract that defines how JSON evidence is named,
changed, tested, and promoted. Without that contract, JSON output can drift by
accident and support-tier claims can overstate what machine consumers may rely
on.

## Behavior Contract

- Every stable command-owned JSON evidence object must include a top-level
  `schema_version` string.
- Schema versions use the form `shipper.<surface>.v<N>`, where `<surface>` is a
  lowercase dot-separated command or artifact surface and `<N>` is a positive
  integer.
- Stable schema names are user-facing contracts. A stable schema name must not
  be reused for breaking changes.
- Additive fields are backward-compatible when existing fields keep their
  meaning, type, and presence rules.
- Removing a field, renaming a field, changing a field type, changing enum
  spelling, or moving a field to a different object is a breaking change.
- Breaking changes require a new schema version and a support-tier update that
  names the proof command for the new version.
- Human-readable output remains the default unless a command explicitly
  documents JSON as its primary output.
- JSON output must keep stdout machine-readable. Human progress, banners, and
  warnings that are not part of the JSON object must go to stderr or to durable
  artifacts.
- JSON output must not invent proof. Unknown, unavailable, skipped, advisory,
  or not-yet-proven facts must be represented explicitly instead of omitted in a
  way that implies success.
- Release artifacts under `.shipper/` are evidence surfaces, not repo-management
  metadata. Repo-goal metadata remains under `.shipper-meta/`.

## Stable JSON Surfaces

These surfaces currently have support-tier proof and may be consumed by agents
or CI under the compatibility rules above:

| Surface | Schema or object | Proof source |
|---|---|---|
| `shipper plan --format json` | `shipper.plan.v1` | `cargo test -p shipper-cli --test bdd_workflow given_multi_crate_when_plan_json_then_valid_json_output` |
| `shipper preflight --format json` | `shipper.preflight.v1` | `cargo test -p shipper-cli preflight` |
| `shipper status --format json` | `shipper.status.v1` | `cargo test -p shipper-cli --test e2e_status status_json_format_produces_registry_report` |
| `shipper status --watch --format json` | `shipper.status.watch.v1` | `cargo test -p shipper-cli status_watch_report_summarizes_state_and_scheduled_events --lib` |
| `shipper doctor --format json` | `shipper.doctor.v1` | `cargo test -p shipper-cli --test e2e_doctor doctor_json_format_reports_diagnostics_without_token_value` |
| `shipper publish --format json` | `shipper.publish.v1` | `cargo test -p shipper-cli --test e2e_publish publish_json_format_writes_command_envelope_to_stdout` |
| `shipper resume --format json` | `shipper.resume.v1` | `cargo test -p shipper-cli --test bdd_resume given_pending_state_when_resume_json_then_stdout_is_command_envelope` |
| `shipper plan-yank --format json` | `shipper.plan_yank.v1` | `cargo test -p shipper-cli --test e2e_expanded --locked plan_yank_json_format_emits_schema_version` |
| `shipper fix-forward --format json` | `shipper.fix_forward.v1` | `cargo test -p shipper-cli --test e2e_expanded --locked fix_forward_json_format_emits_schema_version` |
| `.shipper/remediation-plan.json` from `shipper remediate --dry-run` | `shipper.remediation_plan.v1` | `cargo test -p shipper-cli --test e2e_expanded --locked remediate_dry_run_writes_remediation_plan_artifact` |

The publish and resume JSON rows are command-owned envelopes with nested
receipt evidence, package summaries, and artifact paths.
The remediation command rows are command-owned envelopes with top-level
planning fields plus `schema_version` and `command`. The remediation artifact
row is durable dry-run evidence only; neither surface implies guarded live
execution. Operator-supplied remediation reason text is represented with a
placeholder in durable remediation artifacts.

## Advisory Or Planned JSON Surfaces

- Reconciliation evidence under `.shipper/reconciliation.json` is a product
  target for ambiguity proof. It must not be promoted beyond its implemented
  evidence and tests.
- Remediation plans under `.shipper/remediation-plan.json` use
  `shipper.remediation_plan.v1` once receipt-driven dry-run proof exists. The
  artifact is planning evidence only; it must not imply live yanks, manifest
  edits, or fix-forward publishing.
- Release readiness summaries may link JSON artifacts, but the readiness
  document remains a release artifact, not a schema registry.

## Non-Goals

- Adding new runtime JSON output.
- Generating JSON Schema files for every command.
- Replacing Rust snapshot or integration tests with schema-only validation.
- Changing the receipt schema.
- Changing human output.
- Promoting command envelopes before implementation proof exists.

## Required Evidence

For this spec and plan:

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo xtask package-surface`
- `cargo fmt --all -- --check`
- `git diff --check`

For future stable JSON surfaces:

- a focused test that parses the command output as JSON
- assertions for `schema_version` when the surface is command-owned
- assertions for the fields named in the support-tier proof claim
- snapshot updates only when the serialized contract intentionally changes
- support-tier entries that name the exact proof command

## Acceptance Examples

- Adding `estimated_publish_duration` to `shipper.preflight.v1` is compatible
  when existing fields keep their meaning and tests cover the new field.
- Renaming `schema_version` to `version` in `shipper.plan.v1` is invalid; it
  requires a new schema version.
- Printing a human "Publishing..." banner to stdout before JSON is invalid for
  `--format json`; stdout must remain parseable as the documented JSON object.
- A README claim that "publish JSON is versioned" is valid only because
  `shipper.publish.v1` exists and support tiers name its proof command.
- A command may emit receipt JSON without a command-owned `schema_version` only
  if the support-tier row says the contract is receipt JSON, not a command
  envelope.

## Test Mapping

Current proof is mapped through support tiers:

- plan JSON: `given_multi_crate_when_plan_json_then_valid_json_output`
- preflight JSON: preflight CLI tests
- status JSON: `status_json_format_produces_registry_report`
- status watch JSON: `status_watch_report_summarizes_state_and_scheduled_events`
- doctor JSON: `doctor_json_format_reports_diagnostics_without_token_value`
- publish JSON command envelope:
  `publish_json_format_writes_command_envelope_to_stdout`
- resume JSON command envelope:
  `given_pending_state_when_resume_json_then_stdout_is_command_envelope`

Future implementation PRs should add one focused proof command per promoted
surface before support tiers change.

## Implementation Mapping

The rollout belongs in `plans/0.4.0/json-evidence-contracts.md`.

Implementation PRs should stay narrow:

- docs/spec and plan definition
- active goal manifest update
- one command surface per runtime PR
- support-tier promotion only after proof exists
- optional schema-file generation after command contracts are stable

## CI Proof

CI proof for JSON evidence contracts is initially indirect:

- command tests parse JSON output
- snapshots detect intentional serialized-output changes
- doc-contract checks validate source-of-truth links
- policy-report includes doc-contract status

If JSON Schema files are added later, CI should validate examples against those
schemas before any support-tier promotion depends on them.

## Promotion Rule

A JSON surface can be stable only when:

- the behavior is implemented
- stdout is parseable as documented JSON in `--format json`
- the output has a top-level `schema_version` when command-owned
- a proof command parses and asserts the contract
- `docs/status/SUPPORT_TIERS.md` names that proof command

Publish and resume JSON are promoted as command-owned envelopes
(`shipper.publish.v1` and `shipper.resume.v1`) because their proof commands
assert schema versions, package summaries, artifact paths, and nested receipt
evidence.

## Open Questions

- Should Shipper generate JSON Schema files for stable command-owned surfaces
  after the command contracts settle?
- Which artifacts should carry schema versions independently of command output?
