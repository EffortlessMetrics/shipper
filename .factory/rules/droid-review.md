# Droid Review Rules

> **Status: retired / restoration-only.** No Factory Droid workflow currently
> consumes these rules.

Droid review output is an inter-agent repair queue and inspection record, not a human approval signal.

## Review target

Review changed behavior against:

- AGENTS.md
- CLAUDE.md
- MISSION.md
- ROADMAP.md
- docs/architecture.md
- docs/structure.md
- docs/tech.md
- docs/INVARIANTS.md
- docs/CLIPPY_POLICY.md
- docs/NO_PANIC_POLICY.md
- docs/FILE_POLICY.md
- docs/POLICY_ALLOWLISTS.md
- docs/release-runbook.md
- docs/failure-modes.md
- docs/preflight.md
- docs/readiness.md
- docs/ci/*
- docs/agent-context/review-currentness.md

The currentness document defines shared semantics only. This Factory rule and `.factory/skills/review-guidelines/SKILL.md` are historical restoration inputs, not current executable authority; Claude and Codex use their complete provider-native skills.

## Exact subject

Bind the review to repository, PR number, head SHA, base ref and base SHA, merge-base SHA, synthetic merge/check commit where applicable, Factory skill/rules/model configuration, and relevant tool/schema/fixture/receipt identities. Head-only review is insufficient. If those identities cannot be established, report `NOT_PROVEN`.

Reviewer identity alone does not create independence. State whether the reviewer authored or repaired the current head, what live evidence was reloaded, which external controls were used, correlated-failure risk, and author-side versus independent posture.

## Clean review requirement

Do not emit a naked `LGTM`.

If no actionable findings are emitted, write an inspection record with:

- reviewed effective subject;
- inspected surfaces (concrete files, modules, invariants);
- challenge passes performed;
- existing-finding disposition;
- residual risk;
- validation signal (Observed / Reported / Not verified);
- independence posture;
- `Candidate result: REVIEW_CURRENT`.

## Finding requirement

Before posting, inspect current threads and avoid duplicates. Prefer one review submission with exact-line inline comments anchored to the reviewed head. Use:

```
[P0|P1|P2] title

Failure mode:
Why here:
Fix direction:
Validation:
Confidence:
Evidence: Observed | Reported | Not verified
```

Priorities:

- `P0` — correctness, safety, release, or data-integrity issue.
- `P1` — meaningful risk or contract violation worth fixing before merge.
- `P2` — cleanup, documentation gap, or follow-up acceptable to defer.

`Validation:` names a real local check. Generic phrases like "run tests" are not acceptable.

## Candidate result

Return one substantive result separately from GitHub checks and mergeability:

```text
REVIEW_CURRENT
CHANGES_REQUIRED
NOT_PROVEN
BLOCKED_BY_PREREQUISITE
SUPERSEDED_OR_CLOSE
```

A green check list, mergeability, approval, bot summary, or zero unresolved threads cannot substitute for the candidate result. A failed, cancelled, skipped, rate-limited, malformed, placeholder, or unavailable automated review is unavailable evidence, not a clean review.

## Evidence provenance

Mark each claim:

- `Observed:` directly inspected in this diff or in the listed source files.
- `Reported:` taken from PR body, commits, logs, or another agent.
- `Not verified:` referenced but not confirmed.

Do not treat PR-body claims as independently verified facts.

## Repair and re-review

A valid finding receives a reply naming the fixing commit and focused proof, or an evidence-backed rejection, before resolution. Blanket automated thread resolution is forbidden. After repair, re-review affected findings, dimensions, and repair-created edge cases. Base or merge-base movement can invalidate review without a head change.

Every PR in a stack receives its own candidate result before any campaign synthesis.

## Notification hygiene

Do not @mention users, teams, bots, or organizations.

Do not refer to the PR author by username. Do not address the author in the second person.

Use neutral references: `this PR`, `this diff`, `the changed code`, `the follow-up agent`.

## Shipper priority surfaces

Prioritize, in this order:

1. Registry publish correctness (cargo publish exit semantics, ambiguity classification, retry policy).
2. Ambiguous-outcome reconciliation (registry truth vs cargo stdout).
3. Resume / idempotency / lock behavior.
4. Events / state / receipt coherence (events.jsonl authoritative; state.json projection; receipt.json summary).
5. Token resolution and redaction; Trusted Publishing path.
6. Release workflow behavior (msrv-gate, publish train, dry-run proofs).
7. Public-contract changes across the shipper / shipper-cli / shipper-core boundary.
8. Workflow allowlist, file-policy, clippy-policy, no-panic-policy ledgers.
9. Packaging metadata (description, keywords, categories, license, readme links).
10. Evidence quality of attached tests, snapshots, and proptest seeds.

Do not prioritize style-only comments. Do not prioritize naming preferences absent a concrete failure mode.
