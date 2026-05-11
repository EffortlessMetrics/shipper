# Droid Review Rules

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

## Clean review requirement

Do not emit a naked `LGTM`.

If no actionable findings are emitted, write an inspection record with:

- inspected surfaces (concrete files, modules, invariants);
- checks performed (which lenses were applied);
- why no comments were emitted;
- residual risk (what could still fail in production);
- validation signal (Observed / Reported / Not verified).

## Finding requirement

Use:

```
[P0|P1|P2] title

Failure mode:
Why here:
Fix direction:
Validation:
Confidence:
```

Priorities:

- `P0` — correctness, safety, release, or data-integrity issue.
- `P1` — meaningful risk or contract violation worth fixing before merge.
- `P2` — cleanup, documentation gap, or follow-up acceptable to defer.

`Validation:` names a real local check (e.g., `cargo test -p shipper-core <name>`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`). Generic phrases like "run tests" are not acceptable.

## Evidence provenance

Mark each claim:

- `Observed:` directly inspected in this diff or in the listed source files.
- `Reported:` taken from PR body, commit messages, prior CI logs, or another agent.
- `Not verified:` referenced but not confirmed in this review.

Do not treat PR-body claims as independently verified facts.

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
