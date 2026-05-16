# Plans

Plans are the sequencing layer in Shipper's source-of-truth stack. They turn
accepted proposals, specs, and ADRs into reviewable PR order, proof commands,
rollback notes, and stop conditions.

## Layer Contract

| Layer | Owns | Does not own |
|---|---|---|
| Plan | PR order, proof commands, rollback | product rationale |

Plans should answer:

- What is the end state for this lane?
- Which PR is next?
- What does each PR change in production behavior, docs, policy, or CI?
- Which proof commands must pass for each PR?
- What blocks or unblocks later work?
- How should the work be rolled back or stopped if evidence fails?

## Neighboring Layers

- Proposals explain why the lane exists.
- Specs define what behavior each PR must satisfy.
- ADRs define durable decisions the plan must obey.
- Active goals point Codex and Droid at the current work item in the plan.
- Support tiers are updated only when the plan's proof exists.

## Rules

- Do not redefine behavior that belongs in a spec.
- Do not repeat product rationale that belongs in a proposal.
- Keep PRs small enough to review independently.
- Include proof commands and expected artifacts for every PR.
- If the plan and spec disagree, the spec owns behavior and the plan owns
  sequencing.
