# Plan: Receipt-Driven Remediation

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-19
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/release-readiness-proof.md
Linked issues: #98; #104; #109
Linked PRs: #344; #345
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: no new policy exceptions
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## End State

Shipper remediation is spec-addressable and evidence-backed:

- existing `yank`, `plan-yank`, and `fix-forward` behavior is mapped to proof
  commands before any support-tier promotion
- operators can distinguish containment, fix-forward planning, and future
  guarded execution
- remediation artifacts are named before release claims depend on them
- no PR claims yanking is undo
- support tiers keep full mechanical remediation planned until dry-run artifacts
  and guarded execution are proven

## PR Sequence

### PR 1 - Source-of-truth activation

Linked spec: docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
Blocks: PR 2
Blocked by:
Status: landed in #344

#### Goal

Define the Remediate behavior contract, implementation plan, active goal, and
support-tier guardrails.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Runtime behavior, JSON schema changes, support-tier promotion, live yanks,
release tagging, and publish/release workflow changes.

#### Acceptance

- Spec and plan exist and link to each other.
- `.shipper-meta/goals/active.toml` points to receipt-driven remediation as the
  current lane.
- Support tiers name remediation only as a planned/advisory surface until proof
  commands are mapped.

#### Proof Commands

- `python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/active.toml').read_text()); print('active goal TOML parses')"`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Revert this spec, plan, active goal, and support-tier row if the remediation
lane is superseded before follow-up proof work depends on it.

### PR 2 - Existing remediation proof map

Linked spec: docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
Blocks: PR 3
Blocked by: PR 1
Status: landed in #345

#### Goal

Map already-landed remediation behavior to proof commands and support-tier
claims without changing runtime behavior.

#### Production Delta

Documentation/status only.

#### Non-Goals

New commands, new JSON shape, guarded execution changes, and live yanks.

#### Acceptance

- Support tiers name exact proofs for the bounded remediation surfaces that are
  already implemented.
- Stale issue language is not used as authoritative state.
- Any unproven surface remains advisory or planned.
- Missing proof is explicit: remediation CLI execution, targeted
  `PackageYanked` event proof, stable remediation JSON envelopes, and
  `.shipper/remediation-plan.json` artifact emission remain follow-up work.

#### Proof Commands

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
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Demote any remediation support-tier row whose proof command does not cover the
claim.

### PR 3 - Remediation plan JSON contract

Linked spec: docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
Blocks: PR 4
Blocked by: PR 2
Status: active

#### Goal

Decide whether current `plan-yank` or `fix-forward` JSON is stable enough to
be a command-owned integration contract, or keep it explicitly advisory.

#### Production Delta

Schema/docs/tests only unless a small compatibility field is required.

#### Non-Goals

Executing yanks or publishing fix-forward successors.

#### Acceptance

- `plan-yank --format json` emits a command-owned `shipper.plan_yank.v1`
  envelope with top-level yank-plan fields plus `schema_version` and
  `command`.
- `fix-forward --format json` emits a command-owned `shipper.fix_forward.v1`
  envelope with top-level fix-forward plan fields plus `schema_version` and
  `command`.
- `.shipper/remediation-plan.json` ownership is deferred to PR 4.
- Unknown and operator-supplied facts are explicit.

#### Proof Commands

- `cargo test -p shipper-cli --test e2e_expanded --locked plan_yank_json_format_emits_schema_version`
- `cargo test -p shipper-cli --test e2e_expanded --locked fix_forward_json_format_emits_schema_version`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Demote JSON contract claims if the shape is not stable enough.

### PR 4 - Remediation dry-run artifact

Linked spec: docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
Blocks: PR 5
Blocked by: PR 3

#### Goal

Produce a durable remediation dry-run artifact under `.shipper/` for operator
review and agent consumption.

#### Production Delta

Remediation planning output and artifact writing only.

#### Non-Goals

Live yanks, manifest edits, or fix-forward publish execution.

#### Acceptance

- Dry-run artifact names source receipt, target crate/version, affected
  packages, yank order, fix-forward suggestions, risk notes, and command list.
- Artifact contains no token values.
- Human output points to the artifact path.

#### Proof Commands

- focused CLI/integration tests for artifact generation
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Remove artifact emission if it overclaims or duplicates unstable JSON output.

### PR 5 - Guarded execution proof

Linked spec: docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
Blocks:
Blocked by: PR 4

#### Goal

Prove guarded yank execution against fake Cargo/mock registry surfaces before
any stronger remediation claim is promoted.

#### Production Delta

Execution hardening and tests only.

#### Non-Goals

Publishing fix-forward successors automatically or running live crates.io yanks
in PR CI.

#### Acceptance

- Execution reads a reviewed plan.
- Each yank emits event evidence.
- Retry/backoff and redaction behavior match release safety rules.
- Failures leave actionable state.

#### Proof Commands

- focused fake-Cargo execution tests
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Disable guarded execution paths if fake-registry proof is weak or misleading.
