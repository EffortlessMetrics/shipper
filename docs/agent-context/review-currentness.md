# Pull-request review currentness

This document defines shared semantics for repository review evidence. It is **not an executable review authority**. Claude Code executes `.claude/skills/*`; Codex executes `.agents/skills/*`. Factory Droid is retired, and its retained `.factory/**` files are historical restoration records rather than a current review route. Active provider-native paths must carry the complete procedure they need at the point of work.

## Effective subject

A review is bound to an effective subject, not merely a PR number or head branch:

```text
repository
PR number
head SHA
base ref and base SHA
merge-base SHA
synthetic merge/check commit, where CI evaluates one
review skill/rules identity
relevant tool, schema, fixture, configuration, and receipt identities
```

Head movement invalidates evidence for changed semantics. Base or merge-base movement can change the effective patch while the head remains fixed. A synthetic merge/check commit is valid only for its exact parent identities.

## Candidate judgment and integration posture

Substantive review returns one candidate result:

```text
REVIEW_CURRENT
CHANGES_REQUIRED
NOT_PROVEN
BLOCKED_BY_PREREQUISITE
SUPERSEDED_OR_CLOSE
```

Live GitHub evaluation returns one separate integration posture:

```text
INTEGRATION_READY
PR_IN_FLIGHT
MERGE_BLOCKED
NOT_PROVEN
```

`PR_IN_FLIGHT` is not a review conclusion. `REVIEW_CURRENT` is not proof that checks passed. Neither is publication authorization.

## Review invalidation

A repair invalidates the findings and review dimensions it affects. Re-run affected proof, challenge changed semantic subjects, and re-review the affected dimensions and repair-created edge cases. Do not restart unrelated passes solely because the SHA changed, but do not reuse evidence when head/base/merge-base/rules/configuration movement changes the subject.

## Independence

Reviewer identity alone does not create independence. A useful review record states:

- whether the reviewer authored or repaired the current head;
- what live evidence was reloaded;
- what external repository controls were used;
- what correlated-failure risks remain;
- whether the review is author-side or independent.

An agent that pushes a repair becomes an author of that head. Its earlier review cannot count as independent verification of the repaired candidate.

## Threads and repairs

Inline findings are durable repair records. Valid findings receive a reply naming the fixing commit and focused proof, or an evidence-backed rejection, before resolution. Automated blanket resolution is forbidden. Zero unresolved threads does not prove that a substantive review occurred; a failed, cancelled, skipped, rate-limited, malformed, placeholder, or unavailable reviewer is not a clean review.

## Related PRs

Every PR in a stack or campaign receives its own substantive candidate judgment. A parent synthesis may then review cross-PR schema, identity, authority, artifact-set, limitation-propagation, fan-in, and merge-order contracts. It cannot substitute for child review or become batch approval.
