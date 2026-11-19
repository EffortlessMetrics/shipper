# Explicit manual full-CI hosted proof

Status: accepted
Owner: EffortlessMetrics
Created: 2026-09-07
Milestone: 0.5 candidate proof availability
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md
Linked plan: plans/swarm/development-control-plane.md
Linked issues: #370
Linked PRs: #373
Support-tier impact: no claim promotion; static routing proof does not establish hosted capacity or full-CI success
Policy impact: existing command and authority boundaries retained; no new secrets or release authority
Proof commands: python3 scripts/ci/check-full-ci-hosted.py --test; actionlint -shellcheck=""; cargo xtask check-workflow-surfaces --mode blocking-allowlist; cargo xtask check-process-policy --mode blocking-allowlist; cargo xtask check-network-policy --mode blocking-allowlist; cargo xtask check-file-policy --mode blocking-allowlist

Refs [#370](https://github.com/EffortlessMetrics/shipper-swarm/issues/370).
The inspected base is `5ac0b5723c16ad45c291ff070a293fc455ba44d3`.

## Problem and source truth

The broad CI workflow can wait for unavailable self-hosted allocation without
executing its checks. The current cx43 capacity/group-access cause is unknown.
The existing Rust-small fallback does not prove the complete full-CI job set.
This work adds an explicit manual proof route; it does not diagnose or restore
self-hosted capacity. See [the lane map](../../docs/ci/test-evidence-lanes.md),
[swarm authority](../../docs/status/SWARM_OPERATION.md), and
[workflow policy](../../policy/workflow-allowlist.toml).

## Verified design and scope

- Add a `workflow_dispatch` choice in `.github/workflows/ci.yml`, defaulting to
  `self-hosted`. Only explicit manual `github-hosted` selection in shipper-swarm
  enables the alternative. Push, schedule and default dispatch retain their
  existing runner groups/labels and job predicates.
- Keep the twelve existing command sets as YAML step anchors. The twelve
  hosted counterparts alias those exact steps, including toolchain, command,
  environment and artifact settings; remap only their dependency job IDs.
  Preserve the existing target matrix through an anchor as well.
- Hosted proof uses ephemeral Ubuntu jobs, isolated cache keys and a separate
  concurrency group so a later default main run cannot cancel manual evidence.
  Bound each hosted job's runtime; preserve the ninety-minute crypto budget.
  This broad opt-in run is outside the ordinary PR cost target.
- Keep `contents: read`, no new secrets, and no release workflow changes. The
  release-build job only compiles/uploads an ordinary workflow artifact.
- Add a dependency-free static checker with accept/reject mutation fixtures
  for route exclusivity, the complete job set, shared step authority, runner
  isolation, dependency mapping, matrix and time budgets. Run it from the
  shared policy job and receipt its source in the non-Rust ledger.

## Acceptance and proof

1. Explicit hosted dispatch runs all twelve applicable hosted jobs and no
   original self-hosted jobs. Other events never select a hosted counterpart.
2. Hosted steps are aliases, so commands, test intensity, environment and
   artifacts cannot drift independently. No original job's proof is removed.
3. Hosted DAG edges never depend on excluded self-hosted jobs. Failures remain
   failures; no continue-on-error or skipped proof is promoted to success.
4. Actionlint, the runner guard, focused checker fixtures and existing workflow,
   process/network/file policy checks pass. Do not run local Cargo concurrently
   with the retry repair lane; report deferred checks as not run.
5. After independent review of the exact candidate, root may authorize one
   manual full-CI dispatch. Record exact SHA/run and terminal per-job evidence;
   static checks alone do not prove hosted capacity or full-CI success.

## Non-goals, rollback and cleanup

No automatic fallback/repeated dispatch, runner credential or group changes,
default-hosted push jobs, branch-protection changes, publication, tags, signing,
deployment, or release-authority sync. Issue #370 retains the capacity diagnosis.
Rollback is a normal revert. After merge and post-merge reconciliation, remove
only this lane's clean terminal worktree/branch and temporary evidence files.
