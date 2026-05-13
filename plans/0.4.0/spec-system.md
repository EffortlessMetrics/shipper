# Plan: Shipper 0.4 Source-of-Truth and Release Evidence System

Status: accepted
Owner: EffortlessMetrics
Milestone: 0.4.0-rc.1
Linked proposal: docs/proposals/SHIPPER-PROP-0001-0.4-release-evidence-contract.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md, docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked issues: #109, #195

## End State

Shipper's 0.4 release lane is spec-addressable:

- Source-of-truth stack exists and is documented.
- Templates exist for proposals, specs, ADRs, plans, and goals.
- Support tiers map major Shipper claims to proof commands or artifacts.
- Doc-contract checker exists in advisory mode.
- Policy report includes doc-contract output.
- CI runs doc-contracts advisory.
- 0.4.0 release-readiness proof is implemented using the stack.

## PR Sequence

### PR 1 - Define Source-of-Truth Model

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 2
Blocked by:

#### Goal

Add README files that define each source-of-truth layer.

#### Production Delta

None.

#### Non-Goals

Templates, concrete specs, xtask code, CI changes.

#### Acceptance

- Layer READMEs exist.
- `.shipper-meta/goals/README.md` documents why goals do not live in
  `.shipper/`.

#### Proof Commands

```bash
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
cargo fmt --all -- --check
```

#### Rollback

Remove the new README files and their policy receipts.

### PR 2 - Add Source-of-Truth Templates

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 3
Blocked by: PR 1

#### Goal

Add small templates for proposals, specs, ADRs, plans, and active goals.

#### Production Delta

None.

#### Non-Goals

Concrete source-of-truth artifacts.

#### Acceptance

- Templates include required headers.
- Goal template parses as TOML.

#### Proof Commands

```bash
python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/TEMPLATE.toml').read_text())"
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
cargo fmt --all -- --check
```

#### Rollback

Remove templates and `.shipper-meta` TOML receipt.

### PR 3 - Add Release Evidence Proposal

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 4
Blocked by: PR 2

#### Goal

Add the proposal that frames 0.4 release evidence as a checkable contract.

#### Production Delta

None.

#### Non-Goals

Release proof execution.

#### Acceptance

- Proposal links specs, ADR, plan, issues, and support-tier impact.

#### Proof Commands

```bash
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
```

#### Rollback

Remove the proposal.

### PR 4 - Add Source-of-Truth Spec

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 5
Blocked by: PR 3

#### Goal

Define the behavior contract for the source-of-truth stack.

#### Production Delta

None.

#### Non-Goals

Checker implementation.

#### Acceptance

- Spec defines layer ownership and acceptance examples.

#### Proof Commands

```bash
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
```

#### Rollback

Remove the spec.

### PR 5 - Add Release Readiness Spec

Linked spec: SHIPPER-SPEC-0002
Blocks: PR 12
Blocked by: PR 4

#### Goal

Define the behavior contract for release-readiness artifacts.

#### Production Delta

None.

#### Non-Goals

Executing #195.

#### Acceptance

- Spec lists required readiness evidence and promotion rule.

#### Proof Commands

```bash
cargo xtask policy-report
cargo fmt --all -- --check
```

#### Rollback

Remove the spec.

### PR 6 - Add Support Tiers

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 12
Blocked by: PR 5

#### Goal

Add claim-to-proof map for current Shipper claims.

#### Production Delta

None.

#### Non-Goals

Promoting release readiness before #195 lands.

#### Acceptance

- Major claims are tiered.
- 0.4.0 release readiness remains planned until proof lands.

#### Proof Commands

```bash
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
```

#### Rollback

Remove support-tier map.

### PR 7 - Add Claims-As-State ADR

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 8
Blocked by: PR 6

#### Goal

Record the durable decision that Shipper claims become checkable state.

#### Production Delta

None.

#### Non-Goals

Checker implementation.

#### Acceptance

- ADR records decision, consequences, alternatives, and follow-up specs/plans.

#### Proof Commands

```bash
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
```

#### Rollback

Remove the ADR.

### PR 8 - Add Implementation Plan and Active Goal

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 9
Blocked by: PR 7

#### Goal

Add the active machine-readable goal for the spec-system lane.

#### Production Delta

None.

#### Non-Goals

Doc-contract checker implementation.

#### Acceptance

- `.shipper-meta/goals/active.toml` parses.
- Active goal links the current proposal, specs, plan, issue, and proof
  commands.

#### Proof Commands

```bash
python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/active.toml').read_text())"
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
```

#### Rollback

Remove active goal manifest.

### PR 9 - Add Advisory Doc-Contract Checker

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 10
Blocked by: PR 8

#### Goal

Add `cargo xtask check-doc-contracts --mode advisory`.

#### Production Delta

None.

#### Non-Goals

Blocking enforcement.

#### Acceptance

- Checker writes Markdown and JSON reports.
- Advisory mode exits 0.

#### Proof Commands

```bash
cargo check -p xtask --locked
cargo test -p xtask --locked
cargo xtask check-doc-contracts --mode advisory
cargo xtask policy-report
cargo fmt --all -- --check
cargo clippy -p xtask --all-targets --locked -- -D warnings
```

#### Rollback

Remove the xtask subcommand and reports from policy output.

### PR 10 - Include Doc-Contracts in Policy Report

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 11
Blocked by: PR 9

#### Goal

Make unified policy report include doc-contract results.

#### Production Delta

Policy report gains a new advisory area.

#### Non-Goals

Blocking enforcement.

#### Acceptance

- `target/policy/doc-contracts-report.json` exists.
- `target/policy/policy-report.json` includes doc-contracts.

#### Proof Commands

```bash
cargo xtask check-doc-contracts --mode advisory
cargo xtask policy-report
```

#### Rollback

Remove doc-contract area from policy report.

### PR 11 - Run Doc-Contracts in CI Advisory

Linked spec: SHIPPER-SPEC-0001
Blocks: PR 12
Blocked by: PR 10

#### Goal

Run doc-contracts in the policy CI job and upload reports.

#### Production Delta

CI gains advisory doc-contract evidence.

#### Non-Goals

Branch protection changes.

#### Acceptance

- Policy CI runs `cargo xtask check-doc-contracts --mode advisory`.
- Policy artifacts include doc-contract report files.

#### Proof Commands

```bash
cargo xtask check-doc-contracts --mode advisory
cargo xtask policy-report
```

#### Rollback

Remove CI step.

### PR 12 - Use the System for #195

Linked spec: SHIPPER-SPEC-0002
Blocks: registry reconciliation proposal/spec/ADR/plan
Blocked by: PR 11

#### Goal

Execute #195 and add `docs/release/0.4.0-readiness.md` through the new stack.

#### Production Delta

Release evidence artifact exists.

#### Non-Goals

Publishing or tagging.

#### Acceptance

- #195 gates are run or explicitly recorded as CI-only gaps.
- Support tiers promote 0.4.0 release readiness only when proof exists.
- Active goal is updated or archived.

#### Proof Commands

```bash
cargo xtask check-doc-contracts --mode advisory
cargo xtask policy-report
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo publish --dry-run --workspace
```

#### Rollback

Remove readiness artifact and revert support-tier promotion.
