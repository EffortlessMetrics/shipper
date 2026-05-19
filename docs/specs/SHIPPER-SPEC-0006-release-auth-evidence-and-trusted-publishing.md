# SHIPPER-SPEC-0006: Release Auth Evidence and Trusted Publishing

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-18
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md; docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md; docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/release-auth-evidence-and-trusted-publishing.md
Linked issues: #96; #105; #109
Linked PRs: #338; #340; #342; #360
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for workflow, process, network, and file receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## Problem

Shipper's Harden competency is partially implemented: the release workflow can
mint a crates.io token through Trusted Publishing, Doctor validates visible
workflow prerequisites, preflight warns when token auth wins over an OIDC
context, and token fallback warnings exist. The remaining product gap is not
another generic auth warning. It is durable release evidence that tells an
operator or agent which auth path was actually available, which path was used,
whether a long-lived token fallback remained configured, and what proof exists.

Trusted Publishing also has an external boundary Shipper must not overclaim:
crates.io-side trusted-publisher registration is configured outside this repo.
Shipper can validate visible GitHub workflow prerequisites and observe token
exchange or publish outcomes, but it cannot truthfully claim every crate is
registered until a rehearsal or release proves that fact.

## Behavior Contract

Release auth evidence must preserve these rules:

- Doctor may validate visible workflow prerequisites: `id-token: write`, release
  environment binding, `rust-lang/crates-io-auth-action@v1`, and explicit token
  fallback.
- Doctor must state that crates.io-side trusted-publisher registration is
  external unless a rehearsal or release artifact proves it.
- Preflight must keep OIDC context and token fallback warnings visible when
  Cargo token auth wins.
- Publish, resume, and receipt evidence must eventually expose the auth mode
  Shipper used or observed without logging token values.
- If `CARGO_REGISTRY_TOKEN` is present while GitHub OIDC request variables are
  also present, Shipper must record that observed state as token auth with OIDC
  context unless a separate workflow metadata field proves token provenance.
- Long-lived token fallback must be explicit evidence, not hidden compatibility
  behavior.
- Unknown or externally unproven facts must be represented as unknown,
  advisory, or planned, not omitted.
- Support-tier claims may not call Trusted Publishing the default until proof
  shows the release path prefers the short-lived token and records fallback
  state accurately.
- All evidence must pass through the existing output sanitizer and token values
  must never appear in human output, JSON output, events, receipts, workflow
  logs, or policy artifacts.
- Token non-disclosure is enforced by the `shipper-output-sanitizer` crate and
  by command-specific redaction tests for the evidence surfaces that carry auth
  state.

## Non-Goals

- Implementing crates.io trusted-publisher registration.
- Claiming crates.io-side registration can be proven from repo files alone.
- Removing long-lived token fallback before release rehearsal proves the
  short-lived-token path for every publishable crate.
- Adding receipt signing, SBOM generation, or SLSA provenance in this spec.
- Publishing crates, tagging a release, or changing release versions.
- Changing registry reconciliation behavior.

## Required Evidence

Source-of-truth proof for this spec:

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

Future implementation proof must cover:

- Doctor output for complete, incomplete, and missing Trusted Publishing
  workflow prerequisites
- Preflight output when OIDC context exists but token auth is still used
- Receipt or release evidence that records auth mode and fallback presence
  without token values
- Workflow proof that the Trusted Publishing token mint path ran successfully
  or fell back explicitly
- Support-tier promotion only after the proof command or artifact exists

## Workflow Auth Evidence Artifact

The release workflow must write `.shipper/auth-evidence.json` before any
release-mode `.shipper/` upload that follows Trusted Publishing token minting.
The artifact is intentionally workflow-scoped evidence, not a runtime Shipper
receipt field.

Required fields:

- `schema_version = "shipper.release_auth_evidence.v1"`
- `workflow`
- `job`
- `run_id`
- `run_attempt`
- `commit`
- `environment`
- `auth_action.name = "rust-lang/crates-io-auth-action@v1"`
- `auth_action.outcome`
- `auth_action.token_minted`
- `fallback.configured`
- `fallback.used`
- `selected_token_source`
- `limits`

`selected_token_source` may be `trusted_publishing`, `fallback_secret`, or
`missing`. Token values must never be written. The artifact may prove that the
GitHub-side token mint step succeeded or that fallback was selected; it must not
claim crates.io-side trusted-publisher registration for every publishable crate
unless a later rehearsal or release artifact proves that separately.

## Acceptance Examples

- A release workflow has `id-token: write`, `environment: release`, the crates.io
  auth action, and a fallback secret. Doctor reports the prerequisites as
  visible and the fallback as explicit advisory evidence.
- A workflow exposes only one GitHub OIDC request variable. Doctor reports a
  blocked incomplete OIDC environment and suggests the next action.
- Preflight runs with both OIDC context and `CARGO_REGISTRY_TOKEN`. It records
  that token auth won and warns that fallback is still configured.
- A release receipt records `auth_mode = "trusted_publishing_context"`,
  `auth_mode = "cargo_token"`, or
  `auth_mode = "cargo_token_with_oidc_context"` without storing the token.
  The last value is deliberately an observed context, not a claim that the token
  came from Trusted Publishing or from the fallback secret.
- A support-tier row remains advisory if the proof only validates repo-visible
  workflow files and does not prove crates.io-side registration.

## Test Mapping

Expected implementation proof:

- `cargo test -p shipper-cli --test cli_e2e doctor_command_detects_trusted_publishing_auth --locked`
- `cargo test -p shipper-cli --test cli_e2e doctor_command_warns_when_token_fallback_is_configured --locked`
- `cargo test -p shipper-core run_preflight_warns_when_token_auth_overrides_oidc --lib --locked`
- `cargo test -p shipper-output-sanitizer --locked`
- focused receipt/event tests for auth-mode evidence when that runtime field is
  added
- `cargo test -p shipper-core run_publish_receipt_contains_evidence_after_success --lib --locked`
- `cargo test -p shipper-core event_types_serialize_correctly --lib --locked`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`

## Implementation Mapping

The implementation plan belongs in
`plans/0.4.0/release-auth-evidence-and-trusted-publishing.md`.

The lane should land in narrow PRs:

- source-of-truth activation
- auth mode and token fallback evidence in release artifacts
- Trusted Publishing rehearsal/default proof
- support-tier promotion only after proof exists
- optional migration guide refresh if the proof changes operator steps

## CI Proof

CI should prove unit, integration, BDD, policy, and doc-contract gates for each
implementation PR. A future Trusted Publishing default claim also requires a
workflow artifact proving the short-lived-token path or an explicitly recorded
fallback path; green CI without that artifact is not sufficient.

Current workflow proof:

- Release workflow run `26072938626` (`workflow_dispatch`, `mode=rehearse`,
  `main`) completed successfully without running publish jobs.
- The uploaded `shipper-rehearse-26072938626` artifact contains
  `.shipper/auth-evidence.json`.
- That artifact records `schema_version = "shipper.release_auth_evidence.v1"`,
  `auth_action.outcome = "failure"`, `auth_action.token_minted = false`,
  `fallback.configured = true`, `fallback.used = true`, and
  `selected_token_source = "fallback_secret"`.
- The action log records the actionable external setup gap:
  `No Trusted Publishing config found for repository
  EffortlessMetrics/shipper`.
- This proves fallback evidence is recorded and uploaded. It does not prove
  Trusted Publishing is the default release auth path.

## Promotion Rule

Support-tier claims may move only when the named proof exists:

- Doctor Trusted Publishing diagnostics remain advisory while they only inspect
  visible repo/workflow state.
- Long-lived token fallback warnings remain advisory until release receipts or
  preflight artifacts record fallback state consistently.
- Trusted Publishing default remains planned/advisory until release workflow
  proof shows the short-lived-token path was used for every publishable crate in
  a release-readiness rehearsal or dogfood release, fallback state is recorded,
  and no crate required fallback for successful publication.
- Until the crates.io-side registration proof question is resolved, any claim
  that Shipper crates are registered for Trusted Publishing requires explicit
  per-crate rehearsal or release evidence. The default claim must remain
  planned/advisory without that artifact.

## Open Questions

- Should auth mode be represented as a new receipt field, a preflight proof
  field, an event, or all three?
- Which artifact format should record the per-crate crates.io-side registration
  proof before dogfood release: release-readiness table, workflow artifact,
  receipt field, or all three?
