# Shipper Goal Manifests

Goal manifests are the active execution layer in Shipper's source-of-truth
stack. They give Codex, Droid, humans, and CI a machine-readable pointer to the
current work without scraping stale issue prose or long chat transcripts.

## Namespace Rule

Do not store repo-management goals under `.shipper/`.

`.shipper/` is Shipper's runtime state and artifact namespace. It contains
product state such as `state.json`, `events.jsonl`, receipts, and locks. Mixing
repository-management goals into that namespace would confuse project work with
the product state Shipper writes for users.

Use this namespace instead:

```text
.shipper-meta/goals/active.toml
.shipper-meta/goals/archive/
```

## Layer Contract

| Layer | Owns | Does not own |
|---|---|---|
| Active goal manifest | current machine-readable execution state | historical narrative |

Goal manifests should answer:

- What is the current objective?
- Which work item is ready, active, blocked, or complete?
- Which proposal, spec, plan, issue, and proof commands define the work?
- What end state should be true before the goal changes?

## Neighboring Layers

- Plans define PR sequencing and proof commands.
- Specs define behavior and proof requirements.
- Proposals explain why the goal exists.
- ADRs constrain architecture decisions involved in the goal.
- Support tiers define which user-facing claims may change.
- Policy ledgers receipt exceptions and enforcement state.

## Agent Usage

Before implementing a task, read in this order:

1. `.shipper-meta/goals/active.toml`
2. The linked `plans/...` file for the current work item.
3. The linked `docs/specs/SHIPPER-SPEC-*.md`.
4. The linked proposal if product rationale is needed.
5. The linked ADRs if architecture decisions are involved.
6. The linked policy ledgers if exceptions or receipts are involved.
7. `docs/status/SUPPORT_TIERS.md` before changing README/product claims.

Rules:

- If the active goal and the plan disagree, stop and reconcile.
- If the plan and spec disagree, the spec owns behavior and the plan owns
  sequencing.
- If README claims exceed support tiers, update the claim or the support-tier
  proof.
- If an exception is only in prose, it is not a valid exception.
- If a linked artifact is missing, do not infer it; create or fix the link in a
  separate PR.
- If a spec names an external identifier, command, lint, CLI flag, or API,
  verify it by execution before using it.

## Rules

- Keep active goals small enough for one current lane.
- Archive old goals instead of rewriting history in place.
- Do not use active goals as proposals, specs, ADRs, or release evidence.
- Do not infer missing linked artifacts during implementation; create or fix
  them in a separate PR.
- Top-level goal `status` must be `active`, `blocked`, or `complete`.
- Every work item must set `id` and `status`.
- Work item `status` must be `ready`, `active`, `planned`, `blocked`, or
  `complete`.
- `blocked` work items must name `blocked_by` evidence and a concrete
  `next_action`.
- `ready`, `active`, and `planned` work items must keep proof commands attached
  so current and future work has an explicit validation path.
