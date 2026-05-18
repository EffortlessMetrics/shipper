# SHIPPER-SPEC-0007: Idempotent workspace publish contract

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-18
Supersedes:
Superseded-by:
Linked roadmap pillar: Prove, Reconcile, Integrate
Linked issues: #109
Linked PRs:
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

## 4) Output and evidence requirements

For `shipper publish --format json`, the command envelope must carry:

- A stable schema version (`shipper.publish.v1`).
- A summary surface that distinguishes published vs skipped-existing vs failed.
- Safety signal (`safe_to_rerun`) for idempotent rerun posture.
- Artifact paths for `.shipper/state.json`, `.shipper/events.jsonl`, and
  `.shipper/receipt.json` (plus reconciliation artifact when present).

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
