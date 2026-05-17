# Architecture Decision Records

ADRs are the durable decision layer in Shipper's source-of-truth stack. They
record decisions that should survive beyond one PR, issue, or release train.

## Layer Contract

| Layer | Owns | Does not own |
|---|---|---|
| ADR | durable architecture decision | task list |

ADRs should answer:

- What decision was made?
- What context made the decision necessary?
- What consequences follow for implementation, docs, support tiers, and policy?
- Which alternatives were considered?
- Which specs and plans must obey the decision?

## Neighboring Layers

- Proposals explain why a problem matters.
- Specs define behavior under the decision.
- Plans sequence implementation that obeys the decision.
- Policy ledgers receipt any exceptions created by the decision.

## Rules

- Do not use ADRs as task lists.
- Do not duplicate spec behavior; link the spec and record the decision that
  constrains it.
- Do not bury reversible implementation details here unless they define a
  durable boundary.
- Do not let PR comments be the only record for an architecture rule.
