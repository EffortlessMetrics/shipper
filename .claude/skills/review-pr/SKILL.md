---
name: review-pr
description: Review a pull request substantively and post precise inline findings. Use when asked to review/check a PR, assess merge readiness, review open PRs, or when finish-pr needs a current candidate judgment.
---

# Substantive pull-request review

## Purpose

Review the effective pull-request candidate, not merely the visible patch or a green check list. Produce precise inline findings when defects exist and a useful inspection record when they do not. This skill returns a **candidate judgment**; live CI and mergeability are evaluated later by `verify-live-ci`.

## Immutable review subject

Reload and record the exact effective subject before reviewing:

```text
repository
PR number
head SHA
base ref and base SHA
merge-base SHA
synthetic merge/check commit, when CI evaluates one
review skill/rules identity
relevant tool, schema, fixture, configuration, and receipt identities
```

The head SHA alone is insufficient. Base or merge-base movement can change the effective candidate without moving the head. A synthetic merge/check commit is evidence only for the exact parent identities from which it was created.

If any identity cannot be resolved, return `NOT_PROVEN`; do not infer it from a branch name, PR title, stale local checkout, or earlier review.

## Review posture

Reviewer identity alone does not create independence. State whether this reviewer authored or repaired the current head, which live evidence was reloaded, which external repository controls were used, what correlated-failure risks remain, and whether the review is author-side or independent. An agent that pushes a repair becomes an author of that head; its earlier review cannot count as independent verification.

Do not mutate the candidate while reviewing it. Use read-only workers or subagents for independent lenses where useful, then consolidate before posting. One writer owns any later repair packet.

## Procedure

### 1. Reconstruct the claim

Read the PR body, controlling issue/specification/plan, explicit non-goals, cumulative candidate, commit history, prior review findings, current checks, and relevant repository instructions. Convert the claim into concrete acceptance predicates and identify what the PR says it does **not** prove.

Do not treat PR-body assertions, commit messages, or earlier agent notes as observed fact. Mark evidence as:

```text
Observed
Reported
Not verified
```

### 2. Inspect the complete semantic surface

Inspect every changed file. Then follow relevant semantic owners beyond the diff:

- callers, consumers, adapters, and public entry points;
- schemas, serializers, validators, migrations, fixtures, snapshots, and goldens;
- package, installer, editor, protocol, workflow, release, and support surfaces;
- documentation and claims that can drift from implementation;
- historical defects and nearby code that establish the intended invariant.

Trace the actual production/runtime/protocol/editor/installer/package/release route. Do not stop at a helper merely because the changed function looks locally correct.

### 3. Apply proportionate challenge passes

Try to falsify the candidate across the dimensions it touches:

1. correctness and invariant preservation;
2. architectural ownership and duplicate authority;
3. integration, consumers, compatibility, and platform behavior;
4. test-oracle grip and realistic wrong implementations;
5. security, privacy, credentials, release authority, and claim boundaries;
6. failure, refusal, stale, malformed, unavailable, opposite-direction, rollback, and recovery paths;
7. unnecessary complexity and a materially simpler design.

Use historical defect controls, schema/validator agreement, derived rather than self-attested evidence, artifact/topology/digest verification, and removal experiments when proportionate. A test that passes for the broken implementation is not proof.

### 4. Reconcile existing findings

List current human and bot review threads before posting. Evaluate each against the exact current subject and assign one disposition:

```text
Blocking
NonBlocking
RefutedWithEvidence
DuplicateOrSuperseded
StaleAfterHeadChange
StaleAfterBaseChange
AcceptedFollowUp
RootDecisionRequired
```

A rate-limited, skipped, failed, cancelled, malformed, placeholder, or unavailable automated review is unavailable evidence, not a clean review. Zero unresolved threads does not prove that a substantive review occurred.

### 5. Publish one bounded review

Prefer one review submission containing exact-line inline comments. Before posting each finding, search existing threads and suppress duplicates. Anchor the review to the reviewed head commit.

Use one failure mode per inline thread:

```text
[P0|P1|P2] Short title

Failure mode:
Why here:
Fix direction:
Validation:
Confidence:
Evidence: Observed | Reported | Not verified
```

- `P0`: concrete correctness, safety, release, security, or data-integrity defect.
- `P1`: material contract, integration, or maintainability risk worth fixing before merge.
- `P2`: durable cleanup or bounded follow-up that can be deferred explicitly.

Use a top-level review comment only for a cross-cutting or unanchorable finding. If the authenticated reviewer cannot request changes on its own PR, submit `COMMENT` with explicit blocking language; do not manufacture an approval.

When no actionable findings remain, do not post `LGTM`. Post:

```text
No actionable findings emitted.

Reviewed subject:
Inspected surfaces:
Challenge passes:
Existing-finding disposition:
Residual risk:
Validation signal:
  Observed:
  Reported:
  Not verified:
Independence posture:
Candidate result: REVIEW_CURRENT
```

### 6. Return the substantive result

Return exactly one candidate judgment:

```text
REVIEW_CURRENT
CHANGES_REQUIRED
NOT_PROVEN
BLOCKED_BY_PREREQUISITE
SUPERSEDED_OR_CLOSE
```

`REVIEW_CURRENT` means the reviewed effective subject has no unresolved blocking substantive finding. It does **not** mean checks are terminal, the PR is mergeable, or publication is authorized.

## Repair and re-review

When the result is `CHANGES_REQUIRED`, consolidate valid findings into one repair packet for one writer. The writer must reply to each valid inline thread with the fixing commit and focused proof, or with evidence-backed rejection, before resolution. Blanket automated thread resolution is forbidden.

After repair:

```text
rerun affected proof
→ challenge changed semantic subjects
→ re-review affected findings and dimensions
→ inspect repair-created edge cases
→ update the substantive result
```

Do not restart every review pass solely because the SHA changed, but never reuse review evidence for a semantic subject the repair, base movement, merge-base movement, rules change, or relevant configuration change affected.

## Related and stacked PRs

Review every candidate PR individually. Only after each child has its own current candidate judgment may a parent or campaign synthesize cross-PR schema, identity, authority, artifact-set, limitation-propagation, fan-in, and merge-order contracts. The synthesis is not batch approval and cannot substitute for a child review.

## Claude Code execution mechanics

Use GitHub/`gh` for live PR metadata, diffs, threads, exact identities, and review submission. Use read-only subagents, context forks, or Agent Teams for independent lenses when they improve coverage; do not let reviewer agents edit, commit, push, resolve threads, or change the PR. The coordinating Claude instance owns deduplication, the final inline review, and the candidate result.
