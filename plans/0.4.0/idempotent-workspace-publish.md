# Plan: Idempotent Workspace Publish

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-19
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md; docs/adr/SHIPPER-ADR-0002-registry-truth-over-cargo-output.md
Linked plan: plans/0.4.0/release-readiness-proof.md
Linked issues: #109
Linked PRs: #339
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: no new policy exceptions
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## End State

Shipper publish is a safe, spec-addressable version-idempotent operation:

- packages whose `name@version` already exists on the target registry are
  skipped explicitly
- missing package versions are published in dependency order
- mixed existing/missing workspaces exit successfully only when missing
  packages publish and verify successfully
- ambiguous cargo outcomes reconcile against registry truth before retry
- publish JSON, receipts, state, and events distinguish skipped, published,
  failed, ambiguous, uploaded, and pending packages
- support-tier claims say only what the proof commands and artifacts prove

The claim is registry-version idempotency, not source-diff detection. If a user
changes code without changing a package version, Shipper cannot publish that
change because Cargo registries enforce `name@version` uniqueness.

## PR Sequence

### PR 1 - Source-of-truth activation

Linked spec: docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md
Blocks: PR 2
Blocked by:
Status: active

#### Goal

Create the implementation plan and active goal for idempotent workspace publish
proof, and link the accepted spec to the plan.

#### Production Delta

No runtime behavior change.

#### Non-Goals

Runtime behavior, JSON shape changes, support-tier promotion, release tagging,
and publish workflow changes.

#### Acceptance

- The accepted idempotent workspace publish spec links to this plan.
- `.shipper-meta/goals/active.toml` names this lane as the current execution
  target.
- The support-tier row remains advisory until proof commands exist.

#### Proof Commands

- `python -c "import pathlib,tomllib; tomllib.loads(pathlib.Path('.shipper-meta/goals/active.toml').read_text()); print('active goal TOML parses')"`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Revert this plan, active goal, and spec link if the idempotent publish lane is
superseded before follow-up proof work depends on it.

### PR 2 - Idempotent publish regression suite

Linked spec: docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md
Blocks: PR 3
Blocked by: PR 1
Status: ready

#### Goal

Prove the publish behavior that makes CI reruns safe.

#### Production Delta

Focused tests first. Runtime changes are allowed only if the tests expose a
real contract gap.

#### Non-Goals

New command names, source-diff detection, live crates.io publishing, or
support-tier promotion.

#### Acceptance

- All-existing workspace versions exit `0` and do not invoke `cargo publish`.
- Mixed existing/missing workspace versions skip existing packages and publish
  missing packages in dependency order.
- Real publish failure still exits non-zero and records failure evidence.
- Publish JSON and receipt evidence show skipped and published package states.
- Tests use fake Cargo and mock registry surfaces only.

#### Proof Commands

- `cargo test -p shipper-cli --test bdd_publish --locked`
- `cargo test -p shipper-cli --test e2e_publish --locked`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Revert test/runtime changes if the behavior proves broader than the accepted
spec or weakens publish failure handling.

### PR 3 - Publish rerun posture evidence

Linked spec: docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md
Blocks: PR 4
Blocked by: PR 2
Status: planned

#### Goal

Make the command-owned publish evidence easier for CI, IDPs, and agents to
consume without scraping receipt internals.

#### Production Delta

Additive JSON/human evidence only if PR 2 shows the current
`shipper.publish.v1` envelope is too indirect for rerun posture.

#### Non-Goals

Breaking `shipper.publish.v1`, hiding failures, or claiming source-diff
publish semantics.

#### Acceptance

- Existing package states remain stable.
- Any new field is additive and derived from registry/package outcomes that
  Shipper already proves.
- Unknown/advisory facts are explicit.

#### Proof Commands

- `cargo test -p shipper-cli --test e2e_publish --locked`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Remove additive fields if they overclaim safe rerun behavior or duplicate
receipt authority confusingly.

### PR 4 - Support-tier promotion and user path

Linked spec: docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md
Blocks:
Blocked by: PR 2; PR 3 if needed
Status: planned

#### Goal

Promote the idempotent workspace publish claim only after tests and evidence
prove the contract.

#### Production Delta

Documentation/status only.

#### Non-Goals

Runtime behavior and release/tagging.

#### Acceptance

- `docs/status/SUPPORT_TIERS.md` names exact proof commands.
- User-facing docs describe `shipper publish` as version-idempotent, not
  changed-crate aware.
- The support-tier claim remains narrower than the proof artifacts.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Demote the support-tier row if proof commands do not cover the claim.
