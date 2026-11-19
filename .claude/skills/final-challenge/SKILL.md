---
name: final-challenge
description: Challenge an assembled PR candidate before substantive review. Use from finish-pr and after repairs to expose missing claims, weak proof, duplicate authority, edge cases, and simpler designs.
---

# Final challenge

Take a fresh, adversarial pass over the assembled candidate before review. This is a challenge packet, not an approval and not a duplicate implementation pass.

## Inputs

Reload the controlling issue/spec/plan, cumulative PR claim and non-goals, exact head/base/merge-base identities, changed files, semantic owners, tests and receipts, prior findings, and current repository invariants.

## Challenge

Ask, with concrete evidence:

- What realistic wrong implementation would still pass the supplied tests?
- Which production caller, consumer, platform, schema, package, workflow, or release surface was not exercised?
- Does the change create duplicate authority, bypass an existing owner, or put behavior in the wrong crate/module?
- Are failure, refusal, stale, malformed, unavailable, interruption, rollback, and opposite-direction cases represented?
- Are claims derived from actual artifacts, or self-attested by the code under test?
- Can a materially simpler design satisfy the same predicates with less state or fewer authorities?
- What support, compatibility, security, privacy, credential, or publication boundary could be overstated?
- What changed since any earlier challenge because of repair, base movement, or configuration/rules movement?

Use read-only exploration. Do not edit the candidate. Return a bounded challenge packet grouped as blocking challenge, proof gap, simplification, or no additional challenge. Feed material challenges into `review-pr`; do not post duplicate GitHub threads independently.
