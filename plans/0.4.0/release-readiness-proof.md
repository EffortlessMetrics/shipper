# Plan: 0.4.0 Release Readiness Proof

Status: proposed
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/0.4.0/source-of-truth-stack.md
Linked issues: #109, #195
Linked PRs:
Support-tier impact: docs/status/SUPPORT_TIERS.md
Policy impact: policy ledgers remain authoritative for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report; cargo fmt --all -- --check

## End State

The 0.4.0 release candidate has a committed readiness artifact at
`docs/release/0.4.0-readiness.md`. The artifact records the exact version,
commit, Shipper plan id, gate results, advisory signals, per-crate dry-run
evidence, known carry-over, and sign-off.

This plan does not publish, tag, or implement Reconcile. It makes #195
executable from a spec and plan instead of issue prose.

## Current Plan Snapshot

Run this before executing #195:

```bash
cargo run -p shipper -- plan
```

On 2026-05-13, this command reported:

```text
plan_id: 5d63c5b0725a59a01c1fa1406220808f5a7b1a166c0ddf76d3ba97d13e6feeb5
Total packages to publish: 13
```

The observed order was:

| Order | Crate | Version |
|---:|---|---|
| 1 | shipper-cargo-failure | 0.4.0-rc.1 |
| 2 | shipper-duration | 0.4.0-rc.1 |
| 3 | shipper-encrypt | 0.4.0-rc.1 |
| 4 | shipper-output-sanitizer | 0.4.0-rc.1 |
| 5 | shipper-retry | 0.4.0-rc.1 |
| 6 | shipper-sparse-index | 0.4.0-rc.1 |
| 7 | shipper-webhook | 0.4.0-rc.1 |
| 8 | shipper-types | 0.4.0-rc.1 |
| 9 | shipper-config | 0.4.0-rc.1 |
| 10 | shipper-registry | 0.4.0-rc.1 |
| 11 | shipper-core | 0.4.0-rc.1 |
| 12 | shipper-cli | 0.4.0-rc.1 |
| 13 | shipper | 0.4.0-rc.1 |

If a later `shipper plan` run disagrees, the later command output is
authoritative for the release-readiness proof.

## PR Sequence

### PR 1 - Release-readiness contract

Linked spec: docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Blocks: PR 2
Blocked by: plans/0.4.0/source-of-truth-stack.md

#### Goal

Add this plan and the reusable release-readiness spec.

#### Production Delta

No product runtime behavior change.

#### Non-Goals

Running release dry-runs, tagging, publishing, support-tier promotion, and
Reconcile behavior.

#### Acceptance

- The release-readiness spec defines required evidence.
- This plan defines the #195 gate sequence and dry-run table shape.
- The diff does not add `docs/release/0.4.0-readiness.md`.

#### Proof Commands

- `cargo xtask check-doc-contracts --mode advisory`
- `cargo xtask policy-report`
- `cargo xtask check-file-policy --mode blocking-allowlist`
- `cargo fmt --all -- --check`

#### Rollback

Revert the spec and plan if the release proof contract is replaced.

### PR 2 - Execute #195 release proof

Linked spec: docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Blocks: Reconcile proposal/spec/ADR/plan
Blocked by: PR 1

#### Goal

Produce `docs/release/0.4.0-readiness.md` for `0.4.0-rc.1`.

#### Production Delta

No publish, no tag, and no product runtime behavior change.

#### Non-Goals

Registry reconciliation implementation, release publication, and unrelated
carry-over cleanup.

#### Acceptance

- The readiness document records version, commit SHA, plan id, preflight result,
  policy-report result, advisory lanes, dry-run table, known carry-over, and
  sign-off.
- All publishable crates are dry-run in the authoritative `shipper plan` order.
- `docs/status/SUPPORT_TIERS.md` promotes the release-readiness claim only if
  the required evidence exists.

#### Proof Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo check --workspace
cargo audit
cargo doc --workspace --no-deps --document-private-items
cargo test -p shipper-cli --test bdd_publish
cargo xtask check-doc-contracts --mode advisory
cargo xtask policy-report
```

Dry-run each crate:

```bash
cargo publish --dry-run -p shipper-cargo-failure
cargo publish --dry-run -p shipper-duration
cargo publish --dry-run -p shipper-encrypt
cargo publish --dry-run -p shipper-output-sanitizer
cargo publish --dry-run -p shipper-retry
cargo publish --dry-run -p shipper-sparse-index
cargo publish --dry-run -p shipper-webhook
cargo publish --dry-run -p shipper-types
cargo publish --dry-run -p shipper-config
cargo publish --dry-run -p shipper-registry
cargo publish --dry-run -p shipper-core
cargo publish --dry-run -p shipper-cli
cargo publish --dry-run -p shipper
```

#### Rollback

Revert the readiness artifact and support-tier promotion if the evidence is
wrong. Do not delete independent logs or CI artifacts; attach corrected links in
the replacement readiness artifact.
