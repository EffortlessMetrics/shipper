---
name: deliver-goal
description: Advance an umbrella issue, release outcome, or durable multi-PR campaign through individually reviewed and reconciled pull requests. Use when asked to work issue by issue, PR by PR, finish a campaign, or deliver an end state.
---

# Deliver a durable goal

## Purpose

Coordinate a multi-PR outcome without turning the campaign into batch approval. Read the verbatim goal source when available, current interpretation, constraints, non-goals, acceptance predicates, governing contracts, current `main`, linked issues and PRs, dependencies, recently merged claims, known limitations, and real blockers.

Select the smallest coherent next claim. Do not let an umbrella issue become an excuse for a broad mixed PR.

## Child convergence

Every related candidate must execute the provider-native PR route independently:

```text
candidate assembled
→ provider-native finish-pr
→ final-challenge
→ review-pr
→ substantive candidate result
→ verify-live-ci
→ integration posture
→ merge-reconcile when authorized and ready
```

A parent issue, release train, or campaign summary cannot substitute for child review. Green CI or a clean summary on the umbrella does not approve any child. Every PR in a stack receives its own candidate result: a stacked child is not covered by the review of the PR beneath it, and a result recorded against a base that has since moved is stale, not inherited.

When several PRs are active, maintain a current table:

| PR | Effective identity | Substantive result | Integration posture | Prerequisite | Merge/reconcile state |
| --- | --- | --- | --- | --- | --- |

Respect dependency order. A parent or fan-in cannot outrun a child with `CHANGES_REQUIRED`, `NOT_PROVEN`, `BLOCKED_BY_PREREQUISITE`, `PR_IN_FLIGHT`, or `MERGE_BLOCKED`.

## Campaign synthesis

After each child has its own current candidate judgment, inspect the combined outcome for:

- schema and validator compatibility;
- identity and provenance continuity;
- authority and permission boundaries;
- artifact-set and package completeness;
- limitation and non-claim propagation;
- fan-in assumptions and duplicate ownership;
- merge order, base movement, and stale child review;
- final acceptance predicates that no single PR can prove.

If a merged child moves the base of another candidate, re-evaluate that candidate’s effective subject before relying on its review.

## Progress and closure

Record what landed, what remains, which evidence is current, which claim is still unproved, and the next smallest useful claim. Close the umbrella only when its own acceptance predicates are satisfied by merged state and retained evidence, not merely because all listed PRs closed.

## Authority boundary

This skill coordinates development and review. It never grants tag, crates.io publication, GitHub Release mutation, signing, deployment, credential movement, or release-authority permission. Publication authorization remains a separate explicit release-authority decision.
