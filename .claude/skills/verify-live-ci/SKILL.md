---
name: verify-live-ci
description: Classify live GitHub checks and merge integration for a substantively reviewed PR. Use only after review-pr returns REVIEW_CURRENT and whenever checks, base, mergeability, or conversations change.
---

# Verify live CI and integration

This skill evaluates **integration posture**, not candidate quality. It may run only after the current substantive result is `REVIEW_CURRENT`.

## Reload live state

Resolve and record repository, PR, reviewed head SHA, current base SHA, current merge-base SHA, synthetic merge/check commit where applicable, draft state, mergeability, required checks, workflow runs, review threads, and branch protection/conversation requirements.

If head, base, merge-base, review rules, or relevant configuration moved in a way that affects reviewed semantics, return `NOT_PROVEN` and route back to `review-pr` for affected dimensions.

## Check classification

Classify every required signal explicitly:

```text
success
failed
pending
cancelled
skipped
not_applicable
stale
malformed
unavailable
not_proven
action_required
```

A rate-limited or unavailable reviewer is not a clean review. A skipped route implementation job can be correct only when the repository’s normalized required result proves the selected route. An earlier-head run cannot satisfy the current effective subject.

Green CI alone is not merge readiness. Require terminal required checks on the reviewed subject, a current substantive review, zero unresolved substantive conversations, non-draft posture where required, and actual mergeability. Do not diagnose `BLOCKED` without identifying the active rule or actor.

## Result

Return exactly one integration posture:

```text
INTEGRATION_READY
PR_IN_FLIGHT
MERGE_BLOCKED
NOT_PROVEN
```

- `INTEGRATION_READY`: current review remains valid, required checks are terminal-success or explicitly not applicable, substantive threads are resolved with evidence, and the PR is mergeable.
- `PR_IN_FLIGHT`: valid candidate review exists, but one or more required current-head signals are still legitimately pending.
- `MERGE_BLOCKED`: a named current failure, conversation, draft state, branch rule, conflict, or actor blocks merge.
- `NOT_PROVEN`: identities, receipts, review availability, or check semantics cannot establish the posture.

Never report `INTEGRATION_READY` as publication authorization.
