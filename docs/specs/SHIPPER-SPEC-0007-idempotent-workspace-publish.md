# SHIPPER-SPEC-0007: Idempotent workspace publish contract

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-18
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0003-registry-reconciliation.md; docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md; docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md
Linked ADRs: docs/adr/SHIPPER-ADR-0001-claims-become-checkable-state.md; docs/adr/SHIPPER-ADR-0002-registry-truth-over-cargo-output.md
Linked plan: plans/0.4.0/idempotent-workspace-publish.md
Linked issues: #109
Linked PRs: #339, #355, #356, #357, #358, #359
Support-tier impact: publish contract + CI behavior surface
Policy impact: none
Proof commands: cargo test -p shipper-cli --test bdd_publish --locked; cargo test -p shipper-cli --test e2e_publish --locked; cargo xtask check-doc-contracts --mode advisory; cargo xtask policy-report

## 1) Problem

Users need a CI-safe contract for publishing Rust workspaces where some
`name@version` pairs already exist on the registry and others do not.

The user-facing requirement is:

> Publish only missing workspace package versions, skip versions that already
> exist, and fail CI on real failures.

The engine mostly does this already. The gap is product clarity: command
surface language, explicit exit-code semantics, and a claim-to-proof map
operators can trust.

## 2) Scope and non-goals

### In scope

- Define `shipper publish` as an idempotent, version-based publish action.
- Define exit behavior for mixed states and ambiguous outcomes.
- Define minimum evidence artifacts required for operator and CI trust.
- Define a minimal command chain users can adopt immediately.

### Out of scope

- Changed-crate detection based on local source diffs.
- Version/changelog/tag planning (release-plz territory).
- New top-level command names such as `publish-missing`.

## 3) Contract

Shipper publishes **missing package versions** (registry truth), not
"changed crates" (source diff truth).

- If `foo@1.2.3` does not exist on the target registry, Shipper may publish it.
- If `foo@1.2.3` already exists, Shipper must skip it.
- If code changed without a version bump, Shipper cannot publish that change;
  registry `name@version` uniqueness still applies.

### Minimal operational chain

```bash
cargo install shipper --locked

shipper status
shipper preflight --policy safe
shipper publish --policy safe
```

### Scenario/exit contract

| Scenario | Exit | Required behavior |
|---|---:|---|
| All workspace versions already exist | `0` | Publish nothing; report skipped-existing |
| Some versions exist, some are missing | `0` | Skip existing; publish missing in dependency order |
| Missing crate publishes successfully | `0` | Verify visibility; record receipt |
| Real publish failure | non-zero | Record failure; stop safely |
| Ambiguous cargo result, registry proves published | `0` | Mark published; do not retry blindly |
| Ambiguous cargo result, registry cannot prove outcome | non-zero | Stop before unsafe retry |
| CI interrupted mid-run | interrupted/non-zero | `shipper resume` continues without duplicate publish |

### Retry configuration admission (#362)

The selected release correction for
[#362](https://github.com/EffortlessMetrics/shipper-swarm/issues/362) is explicit
refusal of unsupported per-error configuration. It does not activate the broader
class-policy execution proposed in draft
[#367](https://github.com/EffortlessMetrics/shipper-swarm/pull/367).

- A configured `retry.per_error.retryable`, `.ambiguous`, or `.permanent` class,
  including an empty class subtable, fails with a diagnostic naming
  `retry.per_error` and directing the caller to the global `[retry]` policy.
- Apply the same pure check in config validation and at public engine execution
  boundaries after options are resolved, including direct parallel execution,
  preflight, and rehearsal. Refuse before state/event/receipt mutation, registry
  requests, notifications, or Cargo invocation. A CLI may discover the workspace
  with Cargo metadata before reading its configuration; it must not publish,
  yank, or run a dry-run with the refused configuration.
- Omitted `per_error` and an empty parent table remain valid. CLI retry/force
  flags cannot erase or authorize an unsupported class. Unknown keys within the
  per-error container fail decoding instead of disappearing into an empty value.
- Keep known class records parseable for diagnostics. Preserve the existing
  nested `max_attempts` serde default of zero versus the Rust `Default` value of
  six; neither becomes executable class policy.
- Preserve global retry/CLI precedence, the cumulative package ceiling,
  registry-aware delay floors, terminal automatic `Permanent` handling, and
  `Ambiguous -> reconcile -> retry only NotPublished`. Do not add an event variant,
  class counter, effective class ceiling, or state-schema change for this refusal.

Proof must exercise real config/CLI and direct engine paths, checking unchanged
retained bytes and absent fresh state, zero network/Cargo effects, and positive
omitted/empty controls that still reach global retries. Implementation sequencing
and the retained #367 disposition are in
[the refusal plan](../../plans/swarm/per-error-refusal.md).

## 4) Output and evidence requirements

For `shipper publish --format json`, the command envelope must carry:

- A stable schema version (`shipper.publish.v1`).
- Per-package `packages[].state` values that distinguish published, skipped,
  failed, ambiguous, uploaded, and pending outcomes.
- A top-level `safe_to_rerun` boolean, mirrored by
  `outcome.safe_to_rerun.value` with an explanatory
  `outcome.safe_to_rerun.reason`. The value is true only when the command-owned
  package summary has no pending, uploaded, failed, or ambiguous packages; the
  detailed evidence remains the package states, receipt, and reconciliation
  artifact.
- A pre-receipt controlled stop emits `shipper.publish.error.v1` on stderr,
  keeps stdout empty in JSON mode, and does not claim or synthesize a receipt.
  In this error envelope, its failed-retryable package therefore keeps
  `safe_to_rerun.value: false` with a `safe_to_rerun.reason` that says direct
  `publish` rerun is not authorized. When the exact state, event, and
  `NotPublished` reconciliation evidence is retained and consistent, the
  separate commandless next action may be `resume`. Missing, malformed,
  incomplete, disagreeing, or latest-`StillUnknown` evidence remains
  fail-closed.
- Retry attempts are cumulative per package across publish and resume segments.
  After controlled-stop evidence authorizes resume, a requested
  `--max-attempts` ceiling that is not greater than a selected retryable
  package's retained attempt count must be rejected before a new run segment,
  Cargo publish attempt, registry request, or evidence mutation. Human stderr and
  `shipper.resume.error.v1` JSON stderr identify the package and current,
  requested, and minimum usable ceilings; they preserve
  `safe_to_resume.value: true` and the retained commandless resume action when
  a larger ceiling exists because the rejection does not consume the
  controlled-stop authorization. An exhausted numeric ceiling range is
  fail-closed instead.
- Artifact paths for `.shipper/state.json`, `.shipper/events.jsonl`, and
  `.shipper/receipt.json` (plus reconciliation artifact when present).
- A nested receipt that remains the detailed package-outcome authority.

## 5) Relationship to release-plz and Cargo

- Cargo enforces publish semantics at `name@version` uniqueness and performs
  registry upload.
- release-plz (or equivalent) may decide what changed and bump versions.
- Shipper owns the publish train after versions are already decided:
  prove/readiness, publish, reconcile/survive, and evidence.

## 6) Proof obligations

This claim is only stable when docs, tests, and evidence contract stay aligned.

Minimum proof bundle:

- CLI publish BDD/E2E coverage that includes skipped-existing behavior.
- Reference docs that expose exit-code outcomes for publish/resume/status.
- A CI how-to for "publish missing workspace versions".
- Support-tier entry mapping the claim to proof commands/artifacts.

## 7) Evolution notes

- Do not introduce `publish-missing` unless `shipper publish` semantics change.
- Advisory checksum-based drift detection can be considered later, but it is not
  part of this stable contract.
- Trusted Publishing/auth-evidence lanes are complementary and should remain
  separate from this contract unless a user-facing dependency is introduced.
