# Plan: Shipper 0.4.0-RC.1 Release Readiness Proof

Status: accepted
Owner: EffortlessMetrics
Milestone: 0.4.0-rc.1
Linked proposal: docs/proposals/SHIPPER-PROP-0001-0.4-release-evidence-contract.md
Linked specs: docs/specs/SHIPPER-SPEC-0002-release-readiness-proof.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked issues: #195, #109

## End State

The 0.4.0-rc.1 release candidate has a committed readiness artifact at
`docs/release/0.4.0-readiness.md`. The artifact records local gates, preflight
result, policy state, advisory signals, dry-run publish proof, CI-only gaps,
carry-over, and the explicit boundary that it does not authorize tagging or
publication.

## PR Sequence

### PR 1 - Add Release Readiness Proof

Linked spec: SHIPPER-SPEC-0002
Blocks: registry reconciliation proposal/spec/ADR/plan
Blocked by: source-of-truth stack, doc-contract checker, CI advisory check

#### Goal

Execute #195 gates and add the 0.4.0-rc.1 readiness document through the
source-of-truth stack.

#### Production Delta

No publish behavior changes. The delta is the release evidence artifact and
support-tier promotion for the 0.4.0 readiness proof claim.

#### Non-Goals

- Publishing crates.
- Tagging a release.
- Creating a GitHub release.
- Implementing registry reconciliation.
- Treating advisory ripr or local fuzz gaps as blocking release gates without a
  policy promotion.

#### Acceptance

- `docs/release/0.4.0-readiness.md` exists.
- The readiness document includes version, commit SHA, plan ID, preflight
  result, policy-report summary, lint/no-panic/file-policy state, ripr advisory
  status, dry-run publish table, CI-only gaps, known carry-over, and sign-off.
- `docs/status/SUPPORT_TIERS.md` promotes the release-readiness proof claim only
  because the readiness artifact exists.
- `.shipper-meta/goals/active.toml` names this plan as the current release proof
  work item.

#### Proof Commands

```bash
cargo fmt --all -- --check
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask check-doc-contracts --mode advisory
cargo xtask policy-report
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo check --workspace
cargo audit
cargo doc --workspace --no-deps --document-private-items
cargo test -p shipper-cli --test bdd_publish
cargo publish --dry-run --workspace --allow-dirty --target-dir target/publish-dryrun-workspace
```

#### Rollback

Remove `docs/release/0.4.0-readiness.md`, demote the support-tier claim back to
planned, and return the active goal to the source-of-truth implementation work.
