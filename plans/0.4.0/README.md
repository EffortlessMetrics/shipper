# 0.4.0 Plans

This directory holds implementation plans for the Shipper 0.4 release line.
Plans here sequence release-readiness and source-of-truth work without
duplicating the roadmap, proposal, spec, ADR, or release evidence layers.

## Layer Contract

| Layer | Owns | Does not own |
|---|---|---|
| 0.4.0 plan set | PR sequencing and proof commands for the 0.4 line | product rationale or release evidence |

The 0.4.0 plans should answer:

- Which release-quality PR is next?
- Which spec or proposal authorizes the PR?
- Which proof commands and artifacts are required?
- Which known carry-over items remain outside the PR?
- When should support tiers or release evidence be updated?

## Current Focus

The 0.4 line is moving toward release evidence as a checkable contract. The
release-readiness proof for `0.4.0-rc.1` should become a plan-backed artifact,
not an isolated prose document.

## Rules

- Link to `ROADMAP.md` and #109 for broad release context instead of
  duplicating the nine-competency thesis.
- Link to `docs/release/<version>-readiness.md` for evidence instead of
  copying logs into plans.
- Keep #195 release proof separate from future Reconcile implementation work.
- Do not put active machine-readable goal state in `.shipper/`; use
  `.shipper-meta/goals/active.toml`.
