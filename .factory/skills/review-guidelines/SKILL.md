# Droid Review Guidelines for shipper

> **Status: retired / restoration-only.** No Factory Droid workflow currently
> consumes this file. Codex and Claude use their active provider-native skills.

These reviews are primarily consumed by follow-up coding agents, not by a human reading every comment manually.

Optimize for structured, durable review records. Do not optimize for a low comment count.

## Required context

Before reviewing, use:

- AGENTS.md
- CLAUDE.md
- MISSION.md
- ROADMAP.md
- docs/architecture.md
- docs/structure.md
- docs/tech.md
- docs/product.md
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
- .factory/rules/droid-review.md
- docs/agent-context/review-invariants.md
- docs/agent-context/review-currentness.md

The shared currentness document defines semantics only. This Factory skill is a historical restoration input, not current executable authority; Claude and Codex use their own complete native review skills.

## Product contract

shipper is a reliable, resumable Rust workspace publishing tool. It owns the gap that `cargo publish` and `cargo 1.90`'s multi-package workspace support do not cover: proving readiness, surviving interruptions, reconciling ambiguous registry outcomes, narrating progress, hardening secrets and identity, and producing an audit receipt suitable for incident response.

The product is organized as nine competencies: Prove, Survive, Reconcile, Narrate, Remediate, Harden, Profile, Integrate, Ergonomics. Reconcile is stable and must not regress: an ambiguous `cargo publish` outcome is checked against registry truth and stops on `StillUnknown` rather than blind-retrying.

Review primarily for:

- crates.io publish correctness and idempotency;
- registry visibility and ambiguous-outcome reconciliation;
- resume / state / receipt coherence (events.jsonl is authoritative; state.json is a projection; receipt.json is a summary);
- token resolution and redaction (CARGO_REGISTRY_TOKEN, CARGO_REGISTRIES_<NAME>_TOKEN, $CARGO_HOME/credentials.toml);
- Trusted Publishing path and token fallback;
- output sanitization (shipper-output-sanitizer crate);
- release workflow behavior (msrv-gate, publish train, dry-run proofs);
- three-crate separation: shipper (install façade) / shipper-cli (clap adapter) / shipper-core (engine, no CLI deps);
- workspace-wide `unsafe_code = "forbid"` invariant;
- MSRV 1.95, edition 2024, resolver v3;
- preflight / publish / resume / reconcile contract surfaces;
- CI lane routing and policy ledgers (clippy, no-panic, file-policy, workflow allowlist).

## Exact review subject

Before reviewing, reload and record:

```text
repository
PR number
head SHA
base ref and base SHA
merge-base SHA
synthetic merge/check commit, when CI evaluates one
Factory skill/rules/model configuration identity
relevant tool, schema, fixture, configuration, and receipt identities
```

Head identity alone is insufficient. Base or merge-base movement can change the effective candidate without changing the head. If an identity cannot be resolved, the review is `NOT_PROVEN`; do not infer it from branch names or earlier output.

Reviewer identity alone does not create independence. State whether the reviewer authored or repaired the current head, what live evidence was reloaded, which external repository controls were used, what correlated-failure risks remain, and whether the review is author-side or independent.

## Review posture

A useful review identifies concrete failure modes or records concrete inspection.

Do not suppress actionable findings because there are many of them. Suppress only duplicates, speculation, non-actionable observations, or findings already covered by a clearer comment.

Default to the publishing-safety lens. A small style nit on a CLI flag is less important than a missed retry classification on an HTTP 5xx response, a missed event-write before a destructive step, or a missed token-redaction path.

Inspect every changed file and follow relevant owners, callers, consumers, schemas, fixtures, packages, docs, workflows, and release surfaces beyond the diff. Try to falsify supplied proof with realistic wrong implementations, historical defect controls, failure/refusal/stale/opposite-direction cases, schema/validator disagreement, and removal experiments where proportionate.

## Inline comment format

Before posting, list current threads and avoid duplicates. Prefer one bounded review containing exact-line inline comments anchored to the reviewed head commit. Use a top-level finding only when it is cross-cutting or cannot be anchored.

```
[P0|P1|P2] Short title

Failure mode:
Why here:
Fix direction:
Validation:
Confidence:
Evidence: Observed | Reported | Not verified
```

Priority scale:

- `P0` — correctness, safety, release, or data-integrity issue. Concrete failure mode named.
- `P1` — meaningful risk or contract violation. Worth fixing before merge.
- `P2` — durable cleanup, documentation gap, or follow-up. Acceptable to defer with a tracking note.

`Validation:` should name a real local check (e.g., `cargo test -p shipper-core resume_after_429`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`), not a generic phrase like "run tests".

`Confidence:` should be one of `high`, `medium`, `low`, with one short reason if `medium` or `low`.

## No naked LGTM

If no actionable findings are emitted, the review summary must include:

```
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

`Inspected surfaces:` names the actual files, modules, or invariants examined (e.g., `crates/shipper-core/src/engine/publish.rs:: retry classification`, `events.jsonl write ordering before cargo publish`, `release.yml msrv-gate alignment with Cargo.toml`).

`Residual risk:` names what could still fail in production despite the clean review.

## Evidence provenance

Every claim should be marked:

- `Observed:` — directly inspected in this diff or in the listed source files.
- `Reported:` — taken from the PR body, commit messages, prior CI logs, or another agent's notes.
- `Not verified:` — referenced but not confirmed in this review.

Do not treat the PR body as independently verified fact. A `Reported:` claim that the test suite passes is not a substitute for inspecting the test that was changed.

A rate-limited, skipped, failed, cancelled, malformed, placeholder, or unavailable review is unavailable evidence, not a clean review. Zero unresolved threads does not prove that review occurred.

## Candidate result

Return one substantive result, separately from CI and mergeability:

```text
REVIEW_CURRENT
CHANGES_REQUIRED
NOT_PROVEN
BLOCKED_BY_PREREQUISITE
SUPERSEDED_OR_CLOSE
```

`REVIEW_CURRENT` is not `INTEGRATION_READY` and is never publication authorization.

## Repair-queue posture

Droid review output is consumed by follow-up coding agents as a queue of repairs.

- Use stable, copyable identifiers (file paths, function names, line ranges) so a follow-up agent can find the site.
- Prefer one finding per failure mode over a single comment that lumps three issues together.
- When a finding's fix is unclear, prefer `Fix direction:` over a speculative patch.
- Consolidate valid findings for one writer.
- A valid finding receives a reply naming the fixing commit and focused proof, or an evidence-backed rejection, before resolution.
- Blanket automated thread resolution is forbidden.
- After repair, re-review affected findings, semantic dimensions, and repair-created edge cases. Do not reuse review evidence for semantics changed by head/base/merge-base/rules/configuration movement.

Every PR in a stack receives its own review. A parent synthesis can inspect cross-PR contracts only after child candidate results exist; it cannot become batch approval.

## Notification hygiene

Do not @mention people, teams, bots, or organizations.

Use neutral, PR-scoped references: `this PR`, `this diff`, `the changed code`, `the follow-up agent`.

Do not refer to the PR author by username. Do not address the author in the second person; address the diff.
