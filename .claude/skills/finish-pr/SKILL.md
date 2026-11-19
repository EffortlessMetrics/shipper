---
name: finish-pr
description: Converge an implementation-complete pull request through challenge, substantive review, live CI verification, repair/re-review, and merge reconciliation. Use when asked to finish, land, carry through, get ready, or merge a PR.
---

# Finish pull request

## Trigger point

Use this skill when the candidate implementation and focused local proof are assembled, or whenever the user asks to finish, land, carry through, prepare, or merge a PR. This is the normal convergence entry point; it is not a synonym for “wait for CI.”

## Route

Resolve the current PR and exact live identities, then follow:

```text
candidate assembled
→ final-challenge
→ no useful current substantive review? review-pr
→ REVIEW_CURRENT
→ verify-live-ci
→ INTEGRATION_READY
→ merge-reconcile
```

Green CI, `mergeable: true`, zero unresolved threads, a bot summary, an approval, or the author saying the PR was reviewed cannot bypass `final-challenge` and the provider-native `review-pr` skill.

## Procedure

1. Reload repository, PR, head, base, merge-base, controlling authority, candidate diff, current checks, review threads, and draft/merge state.
2. Ensure the PR body accurately states the cumulative claim, non-goals, semantic owners, risk, proof, and limitations.
3. Invoke the provider-native `final-challenge` skill.
4. Determine whether a substantive review is current for the exact effective subject. If absent, stale, unavailable, shallow, or invalidated, invoke the provider-native `review-pr` skill.
5. If the result is `CHANGES_REQUIRED`, give one writer the consolidated repair packet. After repair, rerun affected proof, invoke `final-challenge` for changed semantic subjects, and invoke `review-pr` for affected findings and dimensions.
6. Proceed only on `REVIEW_CURRENT`. Invoke `verify-live-ci`; do not collapse pending integration into review judgment.
7. Proceed to `merge-reconcile` only on `INTEGRATION_READY` and only when merge was authorized by the request or standing repository workflow.

Stop and report the exact blocker for `NOT_PROVEN`, `BLOCKED_BY_PREREQUISITE`, `SUPERSEDED_OR_CLOSE`, `PR_IN_FLIGHT`, or `MERGE_BLOCKED`. Do not create no-op commits merely to manufacture evidence.

## Stacks and campaigns

Each PR receives its own challenge, substantive review, and integration posture. A campaign summary may be produced only after those child results exist. Respect dependency order; never let a parent or fan-in outrun an unreviewed child.

## Blocking discoveries and repair stacks

Do not merge known-red work into main. Keep technical results truthful: add the
regression that exposes a discovered defect even when the development branch turns
red. Filing an issue, replying to a review, or resolving its thread does not repair it.
A candidate-introduced, worsened, or claim-falsifying defect requires
`CHANGES_REQUIRED`; unavailable evidence stays `NOT_PROVEN`.
Missing, cancelled, stale, partial, or instrument-failed evidence is not a pass.
Retain its cause and return `NOT_PROVEN` until the required evidence is established.

The root selects the smallest useful repair route and one writer per candidate:

- **Candidate defect:** repair the existing PR by default. A child repair may isolate
  useful implementation/review work, but its unsafe parent cannot land first. Fold
  the child into the parent or one combined integration candidate, then prove and
  review the resulting tree before main receives it.
- **Independent prerequisite defect:** reuse its existing repair owner or create one
  bounded repair PR. Dependent work may stack above that repair. After it lands,
  reconcile the dependent delta and refresh affected integration evidence; do not
  duplicate the repair across every blocked PR.
- **Independent non-blocking discovery:** record why the current claim remains true
  and exposure is not worsened, then retain a durable follow-up. An issue URL alone
  cannot establish that classification.

Record the exact parent/head, child-only delta, writer, proof basis, and landing
route for a stack. Parent/child CI is stack-local evidence, not protected-main
acceptance. Never arm child auto-merge into an unprotected feature branch to bypass
that boundary. A contiguous independently safe, reviewed, and proved prefix may
land normally; do not flatten every stack into a giant PR. Preserve unique work
and findings across squash, retarget, and incorporation, and close only the
acceptance actually satisfied by the landed result.

A new material finding withdraws readiness even on an unchanged head. If auto-merge
is armed, disarm it through the available authorized GitHub operation and verify the
readback before resolving the blocking thread. An unavailable or failed disarm is
an integration-control blocker; do not claim the merge is contained. After repair,
affected proof and cumulative rereview must restore `REVIEW_CURRENT` before arming.
Old green, an unrelated later comment, or a completed agent run cannot restore it.

Keep disjoint work moving. For the selected claim, usable repair/proof work outranks
passive waiting; genuine remote waits name their owner and decision-changing event
without an idle polling worker. Batch related findings into one repair wave, preserve
unaffected review, and do not force branch churn or a global intake freeze. These
instructions govern agent decisions; they do not install branch protection.

## Authority boundary

Normal `shipper-swarm` development PRs squash-merge. History-preserving swarm/source synchronization follows its separate merge-commit contract. This skill never authorizes tags, crates.io publication, GitHub Release mutation, signing, deployment, credential movement, or release-authority changes.
