# Droid Review Guidelines for shipper

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

## Product contract

shipper is a reliable, resumable Rust workspace publishing tool. It owns the gap that `cargo publish` and `cargo 1.90`'s multi-package workspace support do not cover: proving readiness, surviving interruptions, reconciling ambiguous registry outcomes, narrating progress, hardening secrets and identity, and producing an audit receipt suitable for incident response.

The product is organized as nine competencies: Prove, Survive, Reconcile, Narrate, Remediate, Harden, Profile, Integrate, Ergonomics. The largest open safety gap is Reconcile — when `cargo publish` exits ambiguously, shipper currently retries instead of reconciling against the registry.

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

## Review posture

A useful review identifies concrete failure modes or records concrete inspection.

Do not suppress actionable findings because there are many of them. Suppress only duplicates, speculation, non-actionable observations, or findings already covered by a clearer comment.

Default to the publishing-safety lens. A small style nit on a CLI flag is less important than a missed retry classification on an HTTP 5xx response, a missed event-write before a destructive step, or a missed token-redaction path.

## Inline comment format

```
[P0|P1|P2] Short title

Failure mode:
Why here:
Fix direction:
Validation:
Confidence:
```

Priority scale:

- `P0` — correctness, safety, release, or data-integrity issue. Concrete failure mode named.
- `P1` — meaningful risk or contract violation. Worth fixing before merge.
- `P2` — durable cleanup, documentation gap, or follow-up. Acceptable to defer with a tracking note.

`Validation:` should name a real local check (e.g., `cargo test -p shipper-core resume_after_429`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo xtask check-pr` once xtask lands), not a generic phrase like "run tests".

`Confidence:` should be one of `high`, `medium`, `low`, with one short reason if `medium` or `low`.

## No naked LGTM

If no actionable findings are emitted, the review summary must include:

```
No actionable findings emitted.

Inspected surfaces:
Checks performed:
Why no comments:
Residual risk:
Validation signal:
  Observed:
  Reported:
  Not verified:
```

`Inspected surfaces:` names the actual files, modules, or invariants examined (e.g., `crates/shipper-core/src/engine/publish.rs:: retry classification`, `events.jsonl write ordering before cargo publish`, `release.yml msrv-gate alignment with Cargo.toml`).

`Why no comments:` explains *why* the diff cleared this lens. "No publish-state or registry-truth changes; only docs", "Pure dependency patch bump; no source touched", etc.

`Residual risk:` names what could still fail in production despite the clean review (e.g., "registry-side ambiguity remains; this PR does not address Reconcile").

## Evidence provenance

Every claim should be marked:

- `Observed:` — directly inspected in this diff or in the listed source files.
- `Reported:` — taken from the PR body, commit messages, prior CI logs, or another agent's notes.
- `Not verified:` — referenced but not confirmed in this review.

Do not treat the PR body as independently verified fact. A `Reported:` claim that the test suite passes is not a substitute for inspecting the test that was changed.

## Repair-queue posture

Droid review output is consumed by follow-up coding agents as a queue of repairs.

- Use stable, copyable identifiers (file paths, function names, line ranges) so a follow-up agent can find the site.
- Prefer one finding per failure mode over a single comment that lumps three issues together.
- When a finding's fix is unclear, prefer `Fix direction:` over a speculative patch. The follow-up agent will write the fix; the reviewer's job is to bound it.

## Notification hygiene

Do not @mention people, teams, bots, or organizations.

Use neutral, PR-scoped references: `this PR`, `this diff`, `the changed code`, `the follow-up agent`.

Do not refer to the PR author by username. Do not address the author in the second person; address the diff.
