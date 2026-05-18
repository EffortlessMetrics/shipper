# Plan: JSON Evidence Contracts

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-18
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/source-of-truth-stack.md
Linked issues: #109
Linked PRs: #315, #316, #317
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: no new policy exceptions
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo xtask package-surface; cargo fmt --all -- --check

## End State

Shipper's machine-readable release evidence has a documented compatibility
contract:

- stable command-owned JSON surfaces carry `shipper.<surface>.v<N>`
  `schema_version` values
- receipt JSON output is named honestly as receipt evidence until command
  envelopes exist
- support tiers name proof commands for each stable JSON claim
- future runtime PRs promote one command surface at a time
- agents can read the spec before changing JSON output

## PR Sequence

### PR 1 - JSON evidence contract spec

Linked spec: docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Blocks: PR 2
Blocked by: #317

#### Goal

Define the naming, compatibility, proof, and support-tier rules for Shipper JSON
evidence contracts.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

New JSON output, schema-file generation, receipt schema changes, active-goal
state, release proof, and product behavior changes.

#### Acceptance

- JSON evidence contract spec exists.
- This plan exists and links to the spec.
- Support tiers link JSON claims back to the spec without promoting new runtime
  behavior.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo xtask package-surface`
- `cargo fmt --all -- --check`
- `git diff --check`

#### Rollback

Revert the spec, plan, and support-tier link if the compatibility policy is
replaced before runtime work depends on it.

### PR 2 - Active goal for release-closure evidence

Linked spec: docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Blocks: PR 3
Blocked by: PR 1

#### Goal

Add `.shipper-meta/goals/active.toml` for the release-closure evidence lane so
agents do not infer work from chat or stale issue text.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Runtime JSON output changes and release proof.

#### Acceptance

- Active goal TOML parses.
- Work items link to this spec and the current plan.
- `.shipper/` remains reserved for runtime release evidence.

#### Proof Commands

- TOML parse check for `.shipper-meta/goals/active.toml`
- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo xtask policy-report`
- `git diff --check`

#### Rollback

Archive or revert the active goal if the lane changes.

### PR 3 - Publish JSON command envelope

Linked spec: docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Blocks: PR 4
Blocked by: PR 2

#### Goal

Decide and implement whether `shipper publish --format json` remains receipt
JSON or gains a command-owned `shipper.publish.v1` envelope with artifact paths
and next-action evidence.

#### Production Delta

CLI output contract change only.

#### Non-Goals

Engine publish behavior changes, registry behavior changes, and receipt schema
changes unless explicitly required by the selected contract.

#### Acceptance

- stdout is parseable JSON.
- tests assert the selected contract.
- support tiers name the exact proof command.

#### Proof Commands

- focused `shipper publish --format json` CLI test
- `cargo clippy -p shipper-cli --all-targets --locked -- -D warnings`
- `cargo xtask policy-report`
- `git diff --check`

#### Rollback

Restore the previous receipt-stdout contract and support-tier text if the
envelope shape is rejected.

### PR 4 - Resume JSON command envelope

Linked spec: docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Blocks: PR 5
Blocked by: PR 3

#### Goal

Give `shipper resume --format json` a contract that answers whether resume is
safe, which packages are complete, what remains, and which artifacts prove it.

#### Production Delta

CLI output contract change only.

#### Non-Goals

Resume engine behavior changes and interruption rehearsal.

#### Acceptance

- stdout is parseable JSON.
- the output reports plan/state/event/receipt artifact paths.
- tests assert safe-to-resume and package-state fields.
- support tiers name the exact proof command.

#### Proof Commands

- focused `shipper resume --format json` CLI test
- `cargo clippy -p shipper-cli --all-targets --locked -- -D warnings`
- `cargo xtask policy-report`
- `git diff --check`

#### Rollback

Restore the previous receipt-stdout contract and support-tier text if the
envelope shape is rejected.

### PR 5 - Schema-file validation, if needed

Linked spec: docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md
Blocks:
Blocked by: PR 3, PR 4

#### Goal

Add generated or hand-maintained JSON Schema files only after the command
contracts settle enough to justify schema validation.

#### Production Delta

Tooling and CI validation only.

#### Non-Goals

New command output.

#### Acceptance

- examples validate against schemas
- CI checks schema/examples
- support tiers name schema validation only for surfaces it actually proves

#### Proof Commands

- future schema validation command
- `cargo xtask policy-report`
- `git diff --check`

#### Rollback

Remove schema validation if it creates churn before command contracts settle.
