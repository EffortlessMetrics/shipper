# Plan: Release Auth Evidence and Trusted Publishing

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-18
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/release-operator-visibility-and-survive-proof.md
Linked issues: #96; #105; #109
Linked PRs: #338; #340; #342; #360
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: no new policy exceptions
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## End State

Shipper release auth is visible, bounded, and evidence-backed:

- Doctor tells operators which Trusted Publishing prerequisites are visible and
  which remain externally unproven.
- Preflight, publish, resume, events, and receipts expose whether the release
  used Trusted Publishing, token fallback, or an unknown/auth-missing path.
- Long-lived token fallback remains available for bootstrap and incident
  response, but is never hidden.
- Support tiers distinguish advisory diagnostics from a proven Trusted
  Publishing default.
- No auth evidence stores token values.

Existing foundations, including Doctor JSON diagnostics, preflight OIDC
warnings, release workflow OIDC wiring, and token sanitizer coverage, are
treated as already landed baseline.

## PR Sequence

### PR 1 - Source-of-truth activation

Linked spec: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Blocks: PR 2
Blocked by:

#### Goal

Define the behavior contract, implementation plan, active goal, and
support-tier guardrails for release auth evidence and Trusted Publishing.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Runtime behavior, release workflow changes, support-tier promotion, crates.io
publishing, release tagging, receipt signing, and SBOM generation.

#### Acceptance

- Spec and plan exist and link to each other.
- `.shipper-meta/goals/active.toml` points to release auth evidence as the
  current lane.
- Support tiers remain advisory/planned and do not claim Trusted Publishing is
  the default.

#### Proof Commands

- `python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/active.toml').read_text()); print('active goal TOML parses')"`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Revert this spec, plan, active goal, and support-tier references if the auth
lane is superseded before runtime work depends on it.

### PR 2 - Auth mode evidence

Linked spec: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Blocks: PR 3
Blocked by: PR 1

#### Goal

Record auth mode and token fallback presence in release evidence without
exposing token values.

#### Production Delta

Preflight, publish/resume evidence, events, or receipts only, depending on the
smallest existing data path that can truthfully carry the fact.

#### Non-Goals

Trusted Publishing default promotion, crates.io-side registration proof,
release workflow rewrites, and receipt signing.

#### Acceptance

- Evidence distinguishes Trusted Publishing context, Cargo token auth, Cargo
  token auth with OIDC context, missing auth, and unknown auth paths.
- Token fallback remains visible when workflow metadata or Doctor-visible
  workflow configuration proves it; token-plus-OIDC runtime evidence must not
  overclaim token provenance by itself.
- Token values never appear in human output, JSON output, events, receipts, or
  snapshots.
- Unknown crates.io-side registration remains explicit.

#### Proof Commands

- `cargo test -p shipper-core run_preflight_warns_when_token_auth_overrides_oidc --lib --locked`
- `cargo test -p shipper-core collect_auth_evidence --lib --locked`
- `cargo test -p shipper-core run_publish_receipt_contains_evidence_after_success --lib --locked`
- `cargo test -p shipper-output-sanitizer --locked`
- focused receipt/event tests for auth-mode evidence
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Remove auth-mode evidence fields if they overclaim or leak sensitive values.

### PR 3 - Trusted Publishing proof artifact

Linked spec: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Blocks: PR 4
Blocked by: PR 2

#### Goal

Produce a workflow or release-readiness artifact that proves whether the
Trusted Publishing token mint path works for Shipper's release environment and
whether fallback was used.

#### Production Delta

Release workflow/rehearsal evidence only. The release workflow writes
`.shipper/auth-evidence.json` after each Trusted Publishing token mint step and
before the following `.shipper/` artifact upload.

#### Non-Goals

Publishing crates unless this runs as part of an approved release proof,
removing fallback, or claiming crates.io-side registration for crates that were
not rehearsed.

#### Acceptance

- Artifact records whether `rust-lang/crates-io-auth-action@v1` succeeded.
- Artifact records whether token fallback was used.
- Artifact names the workflow, run ID, commit, and environment.
- Failed Trusted Publishing registration remains an actionable setup gap, not a
  generic auth failure.
- Artifact limits state that token values are omitted and crates.io-side
  trusted-publisher registration is externally unproven unless separately
  rehearsed.
- Release workflow run `26072938626` uploaded `shipper-rehearse-26072938626`
  with `.shipper/auth-evidence.json`; the artifact records token mint failure,
  fallback configured, fallback used, and `selected_token_source =
  "fallback_secret"`.

#### Proof Commands

- release auth rehearsal or release-readiness workflow artifact
- `cargo xtask check-workflow-surfaces --mode advisory`
- `cargo xtask check-process-policy --mode advisory`
- `cargo xtask check-network-policy --mode advisory`
- `cargo xtask policy-report`
- `git diff --check`

#### Rollback

Demote any Trusted Publishing claim if the artifact is missing or proves only
fallback behavior.

#### Result

Complete. The first proof artifact exists and proves fallback behavior for the
current release environment. It intentionally does not promote Trusted
Publishing as the default, because crates.io returned `No Trusted Publishing
config found for repository EffortlessMetrics/shipper`.

### PR 4 - Support-tier promotion

Linked spec: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Blocks:
Blocked by: PR 3

#### Goal

Promote only claims proven by auth-mode evidence and workflow artifacts.

#### Production Delta

Documentation/status only.

#### Non-Goals

Runtime changes and release workflow changes.

#### Acceptance

- Support tiers name exact proof commands and artifacts.
- Trusted Publishing default remains planned/advisory unless short-lived-token
  use is proven as the normal path.
- README and product claims do not exceed proven evidence.

#### Result

Complete for the bounded proof-artifact claim. Support tiers now name the
workflow artifact proof and keep the stronger Trusted Publishing default claim
planned/advisory.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Demote any claim whose proof artifact is missing or weak.

### PR 5 - Trusted Publishing default proof activation

Linked spec: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Blocks: PR 6
Blocked by: PR 4

#### Goal

Move the active goal from completed idempotent publish proof to the remaining
Trusted Publishing default proof gap, and archive the completed goal.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Trusted Publishing promotion, crates.io-side registration changes, release
publication, release tagging, and workflow credential changes.

#### Acceptance

- `.shipper-meta/goals/active.toml` points to this accepted spec and plan.
- The completed idempotent workspace publish goal is archived under
  `.shipper-meta/goals/archive/`.
- Support tiers continue to say that Trusted Publishing default is
  planned/advisory until short-lived-token proof exists.
- The plan distinguishes the existing fallback proof artifact from the future
  default proof artifact.

#### Result

Complete in #360. The active goal now points to this release-auth spec and plan,
the idempotent workspace publish goal is archived, and Trusted Publishing
default remains planned/advisory until short-lived-token proof exists.

#### Proof Commands

- `python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/active.toml').read_text()); print('active goal TOML parses')"`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Restore the completed idempotent active goal if the Trusted Publishing default
proof lane is deferred before follow-up work depends on it.

### PR 6 - Trusted Publishing default proof artifact

Linked spec: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Blocks: PR 7
Blocked by: PR 5 and external crates.io trusted-publisher registration

#### Goal

Produce release evidence proving whether the short-lived-token path is the
normal release auth path and whether fallback remained unused.

#### Production Delta

Release rehearsal or release-readiness evidence only.

#### Non-Goals

Removing fallback, publishing without explicit release approval, hiding fallback
configuration, or claiming crates.io registration from repo files alone.

#### Acceptance

- The evidence artifact records `selected_token_source = "trusted_publishing"`
  for the rehearsed release path, or records the exact external blocker.
- Fallback configured/used state is explicit.
- The artifact names workflow, run ID, commit, environment, and limits.
- Token values are omitted.
- Any crates.io-side registration gap remains explicit.

#### Current Blocker

Blocked on external crates.io trusted-publisher registration evidence. The
latest recorded release auth proof is workflow run `26072938626`, which uploaded
`.shipper/auth-evidence.json` with `selected_token_source =
"fallback_secret"` and the setup gap `No Trusted Publishing config found for
repository EffortlessMetrics/shipper`.

#### Proof Commands

- release auth rehearsal or release-readiness workflow artifact
- `cargo xtask check-workflow-surfaces --mode advisory`
- `cargo xtask check-process-policy --mode advisory`
- `cargo xtask check-network-policy --mode advisory`
- `cargo xtask policy-report`
- `git diff --check`

#### Rollback

Demote or leave unpromoted the Trusted Publishing default claim if the artifact
proves fallback, missing registration, or any unknown auth path.

### PR 7 - Trusted Publishing default support-tier promotion

Linked spec: docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md
Blocks:
Blocked by: PR 6

#### Goal

Promote the Trusted Publishing default claim only if the release evidence proves
the short-lived-token path.

#### Production Delta

Documentation/status only.

#### Non-Goals

Runtime changes, workflow changes, release publication, or release tagging.

#### Acceptance

- `docs/status/SUPPORT_TIERS.md` names the exact proof artifact and commands.
- The promoted claim remains limited to the rehearsed/proven release path.
- Token fallback remains documented as explicit incident-response behavior if
  it remains configured.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Demote the claim if the proof artifact is missing, weak, or proves fallback.
