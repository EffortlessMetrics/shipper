# Swarm Development Control Plane

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-24
Milestone: swarm-control-plane
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: none
Linked issues: none
Linked PRs: none
Support-tier impact: source of truth only; no product claim promotion
Policy impact: preserves swarm/source release-authority boundary
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check; git diff --check

## Objective

Keep `EffortlessMetrics/shipper-swarm` operating as Shipper's active
development control plane while `EffortlessMetrics/shipper` remains the release
authority.

This plan is not a new product feature lane. It is the execution lane agents
use when the PR queue is empty or ambiguous.

## Operating Loop

1. Refresh reality first: open PRs, `origin/main`, CI runs, review findings,
   branch ancestry, active goal, and support tiers.
2. Finish the existing queue before opening overlapping work.
3. Keep normal swarm PRs SRP-sized and squash-merge only after the normalized
   required check and review signals are clean.
4. Keep release credentials, crates.io publish authority, tags, signing, and
   provenance in `EffortlessMetrics/shipper`.
5. Do not sync swarm-only runner policy back to the release-authority repo
   until that boundary is explicitly decided.
6. When the queue is empty, choose the next smallest improvement from
   repo-local source-of-truth artifacts.

## Current Work Items

### Queue Stewardship

Inspect every Codex Web, Claude Code Web, Droid, Dependabot, and human PR as
part of the active queue. Fix real findings, validate honestly, merge clean
normal swarm PRs by squash, and clean up branches and temporary artifacts.

### Post-Merge Health

After each merge, verify post-merge `main` evidence. If main CI fails, treat
that failure as the next queue item before starting unrelated work.

### Source-of-Truth Hygiene

Keep `.shipper-meta/goals/active.toml`, `docs/status/SWARM_OPERATION.md`,
`docs/status/SWARM_SYNC.md`, `docs/ci/test-evidence-lanes.md`, and
`docs/status/SUPPORT_TIERS.md` aligned with the actual repo state. The
doc-contract checker now verifies that the workflow inventory in
`docs/ci/test-evidence-lanes.md` covers every tracked
`.github/workflows/*.yml` file and rejects stale inventory entries.

## Not In Scope

- Moving crates.io tokens or signing credentials into `shipper-swarm`.
- Promoting Trusted Publishing default without release-authority proof.
- Syncing the all-self-hosted swarm CI policy back to `EffortlessMetrics/shipper`.
- Broad cleanup that does not reduce release risk, CI confusion, or agent
  handoff ambiguity.

## Proof

Minimum proof for source-of-truth changes in this lane:

```bash
cargo xtask check-doc-contracts --mode advisory
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask policy-report
cargo fmt --all -- --check
git diff --check
```

When workflow files change, also run:

```bash
cargo xtask check-workflow-surfaces --mode blocking-allowlist
cargo xtask check-process-policy --mode blocking-allowlist
cargo xtask check-network-policy --mode blocking-allowlist
```
