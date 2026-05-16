# Proposals

Proposals are the "why" layer in Shipper's source-of-truth stack. They explain
the user value, alternatives, success criteria, risks, and evidence strategy
for a product or operating-system change.

## Layer Contract

| Layer | Owns | Does not own |
|---|---|---|
| Proposal | why, user value, alternatives, success criteria | PR sequencing |

Proposals should answer:

- What user or operator problem is being solved?
- Why does this belong in Shipper instead of Cargo, CI glue, or prose docs?
- What outcomes would make the proposal successful?
- What alternatives were rejected, and why?
- Which specs, ADRs, plans, issues, and support-tier claims need to exist next?

## Neighboring Layers

- Specs define the behavior contract and proof requirements implied by a
  proposal.
- ADRs record durable decisions when the proposal creates an architecture rule.
- Plans sequence PRs and proof commands after the proposal's direction is
  accepted.
- Support tiers say which user-facing claims may be made from the resulting
  evidence.

## Rules

- Do not put PR-by-PR sequencing here; put it in `plans/`.
- Do not duplicate policy ledgers; link to `policy/*.toml` and proof commands.
- Do not duplicate release evidence; link to `docs/release/<version>-readiness.md`.
- Do not duplicate roadmap text; link to `ROADMAP.md` or the relevant issue
  when broader context is needed.
