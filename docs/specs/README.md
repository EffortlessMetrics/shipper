# Specs

Specs are the "what must be true" layer in Shipper's source-of-truth stack.
They define behavior contracts, non-goals, required evidence, acceptance
examples, test mapping, and promotion rules.

## Layer Contract

| Layer | Owns | Does not own |
|---|---|---|
| Spec | behavior contract, non-goals, proof requirements | campaign narrative |

Specs should answer:

- What behavior must hold for users, operators, agents, and CI?
- What evidence proves the behavior?
- Which commands or artifacts are required before a claim can be promoted?
- What is out of scope?
- Which plan implements the behavior, and which proposal explains why?

## Neighboring Layers

- Proposals explain why the behavior matters.
- ADRs preserve durable architecture decisions that constrain the behavior.
- Plans define the PR order and rollout mechanics.
- Support tiers decide whether README or product claims may be stable,
  advisory, experimental, or planned.
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

- Do not use specs for PR sequencing; put sequencing in `plans/`.
- Do not use specs for product rationale; link the proposal.
- Do not use specs for permanent exception records; put receipts in
  `policy/*.toml`.
- Do not promote a public claim from a spec alone; update the support-tier map
  when the proof exists.
