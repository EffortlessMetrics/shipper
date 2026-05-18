# Plan: Source-of-Truth Stack

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan:
Linked issues: #109, #195
Linked PRs: #239, #240, #241, #242, #243, #244, #250, #251, #252, #253, #254, #255, #319
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: doc-contract advisory report and policy-report integration
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check

## End State

Shipper's source-of-truth stack is executable by maintainers and agents:

- proposals explain why a lane exists
- specs define behavior and proof
- ADRs record durable decisions
- plans define sequencing, rollback, and proof commands
- active goals define current machine-readable execution
- support tiers map claims to proof commands and artifacts
- policy ledgers remain authoritative for exceptions and receipts

The stack reached its first release-proof use when #195 was executed from the
release-readiness spec and plan instead of issue prose. Later work may harden
the checker from advisory to blocking once the reports have burned in.

## PR Sequence

### PR 1 - Source-of-truth scaffold

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 2
Blocked by:

#### Goal

Define the layer README scaffold.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Templates, concrete proposal/spec/ADR files, support tiers, active goals,
checker code, CI wiring, and #195 release proof.

#### Acceptance

Merged as #239.

#### Proof Commands

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the scaffold docs and policy receipt if the layer contract is rejected.

### PR 2 - Templates

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 3
Blocked by: PR 1

#### Goal

Add small templates for proposals, specs, ADRs, plans, and active goals.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Concrete lane artifacts, support tiers, active goal state, checker code, CI
wiring, and #195 release proof.

#### Acceptance

Merged as #240.

#### Proof Commands

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- TOML parse check for `.shipper-meta/goals/TEMPLATE.toml`

#### Rollback

Revert templates and their policy receipt.

### PR 3 - Source-of-truth proposal

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 4
Blocked by: PR 2

#### Goal

Explain why Shipper needs a claim-to-proof source-of-truth stack.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Spec behavior, ADR decisions, support-tier map, active goal state, checker code,
CI wiring, and #195 release proof.

#### Acceptance

Merged as #241.

#### Proof Commands

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the proposal if the lane rationale changes.

### PR 4 - Source-of-truth spec

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 5
Blocked by: PR 3

#### Goal

Define the source-of-truth stack behavior contract.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

PR sequencing beyond references to this plan, support-tier map, active goal
state, checker code, CI wiring, and #195 release proof.

#### Acceptance

Merged as #242.

#### Proof Commands

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the spec if the behavior contract is replaced.

### PR 5 - Support tiers

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 6
Blocked by: PR 4

#### Goal

Add the first claim-to-proof map for Shipper product and internal claims.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

README claim promotion, release proof execution, checker code, and CI wiring.

#### Acceptance

Merged as #243.

#### Proof Commands

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the support-tier file if the tier model is replaced.

### PR 6 - Claims become checkable state ADR

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 7
Blocked by: PR 5

#### Goal

Record the durable decision that stable claims require proof.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Active goal state, checker code, CI wiring, #195 release proof, and Reconcile
behavior.

#### Acceptance

Merged as #244.

#### Proof Commands

- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the ADR if the durable decision is superseded.

### PR 7 - Implementation plan and active goal

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 8
Blocked by: PR 6

#### Goal

Add this plan and `.shipper-meta/goals/active.toml` so future agents can start
from a machine-readable execution target.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Doc-contract checker code, policy-report integration, CI wiring, #195 release
proof, and Reconcile behavior.

#### Acceptance

Merged as #250.

The active goal was later refreshed for the current source-of-truth lane in
#319.

#### Proof Commands

- `python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/active.toml').read_text()); print('active goal TOML parses')"`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert this plan and active goal if the execution target changes.

### PR 8 - Advisory doc-contract checker

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 9
Blocked by: PR 7

#### Goal

Add `cargo xtask check-doc-contracts --mode advisory`.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Blocking enforcement, CI wiring, policy-report integration, release proof, and
Reconcile behavior.

#### Acceptance

Merged as #251.

#### Proof Commands

- `cargo check -p xtask --locked`
- `cargo test -p xtask --locked`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `cargo clippy -p xtask --all-targets --locked -- -D warnings`

#### Rollback

Revert the xtask command and reports if the checker model is replaced.

### PR 9 - Policy-report integration

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 10
Blocked by: PR 8

#### Goal

Include doc-contract status in `cargo xtask policy-report`.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

CI wiring, blocking doc-contract mode, release proof, and Reconcile behavior.

#### Acceptance

Merged as #252.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `cargo clippy -p xtask --all-targets --locked -- -D warnings`

#### Rollback

Remove doc-contract status from policy report.

### PR 10 - CI advisory

Linked spec: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Blocks: PR 11
Blocked by: PR 9

#### Goal

Run doc-contract checks in CI advisory mode and upload reports.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Blocking enforcement, release proof execution, and Reconcile behavior.

#### Acceptance

Merged as #253.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`

#### Rollback

Remove the CI advisory step and artifact upload.

### PR 11 - Release-readiness spec and plan

Linked spec: docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Blocks: PR 12
Blocked by: PR 10

#### Goal

Define the reusable release-readiness proof contract and #195 execution plan.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Running dry-run publish, tagging, publishing, and Reconcile behavior.

#### Acceptance

Merged as #254.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`

#### Rollback

Revert the spec and plan if the release proof contract is replaced.

### PR 12 - Execute #195 release proof

Linked spec: docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Blocks: Reconcile proposal/spec/ADR/plan
Blocked by: PR 11

#### Goal

Produce `docs/release/0.4.0-readiness.md` from the release-readiness contract.

#### Production Delta

No publish, no tag, no runtime behavior change.

#### Non-Goals

Registry reconciliation implementation and release publication.

#### Acceptance

Merged as #255.

#### Proof Commands

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo test --workspace --doc`
- `cargo check --workspace`
- `cargo audit`
- `cargo doc --workspace --no-deps --document-private-items`
- `cargo test -p shipper-cli --test bdd_publish`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`

#### Rollback

Revert the readiness artifact and support-tier promotion if evidence is wrong.
