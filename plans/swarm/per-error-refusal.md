# Refuse unsupported per-error retry configuration

Status: implemented
Owner: EffortlessMetrics
Created: 2026-10-29
Milestone: 0.5 release candidate preparation
Linked proposal:
Linked specs: docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md
Linked ADRs:
Linked plan: plans/swarm/build-readiness.md
Linked issues: #362; #255
Linked PRs: #439
Support-tier impact: no promotion; selected per-error configuration refusal implemented in development source, not public 0.5 availability
Policy impact: metadata contract repair only; no workflow, authority or exception changes
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask check-file-policy --mode blocking-allowlist; git diff --check

## Current disposition — 2026-11-02

PR [#439](https://github.com/EffortlessMetrics/shipper-swarm/pull/439)
landed the selected fail-closed refusal for configured `retry.per_error`
class entries, including empty class subtables; issue #362 is closed.
Omitted settings and an empty parent table remain valid. The richer per-error
execution design in draft #367 was **not** adopted. The sections below
preserve the original October 29 investigation and proposed implementation
sequence as **historical plan evidence**, not as an outstanding development
work queue or a claim of a newly qualified public release.

Remaining release acceptance belongs to #255 and the distinct
release-authority process. **Candidate: NOT FROZEN.
Publication authorization: NO.**
Issue: [#362](https://github.com/EffortlessMetrics/shipper-swarm/issues/362)
Contract: [SPEC-0007 retry admission](../../docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md#retry-configuration-admission-362)
Development target: `EffortlessMetrics/shipper-swarm/main`

## October 29 selected release slice and recovered work (historical)

The 2026-10-29 development task selects the issue's recommended minimal refusal
correction for this release. The candidate records that selection for independent
review; it does not claim that review, main integration, release freeze, or
publication has completed. The controlling issue's
[September 22 review](https://github.com/EffortlessMetrics/shipper-swarm/issues/362#issuecomment-5772946300)
contains the rationale and alternatives; SPEC-0007 owns the selected behavior.

Draft [#367](https://github.com/EffortlessMetrics/shipper-swarm/pull/367) was
recovered at `2f369b06742ab48fad19eaba1f599c71906c9d57`, including prior repair
`507db7d535b3997d4c830e375f8157df2ec450de` and merged atomic-batch prerequisites.
Its full class policies, automatic Permanent retry opt-in, and completion event
remain unadopted for this release. Preserve that branch and its original proof as
historical work; its focused Windows 77-pass result is not proof of this refusal
candidate. The coordinator owns the issue/PR disposition after independent review.
No #367 commits are imported into this leaf.

## Implementation and integration order

1. Start from main `c2ae7d20528e91ca771c03e74576bbf726ff7b68` in an isolated
   worktree. Share one pure refusal check between config validation and public
   engine boundaries. Preserve the config/runtime adapter's lossless transport
   and diagnostic loader behavior.
2. Remove generated class examples, replace the unsupported documentation claim,
   document migration/default compatibility, and add a Changie fragment.
3. Prove refusal through real CLI/file loading and direct public API entry points,
   including fresh and retained state, both scheduler choices, and force flags.
   Preserve global retry execution with omitted/empty controls and run the existing
   relevant Permanent and ambiguous-outcome regressions.
4. Route the local committed candidate through repository-native `finish-pr`,
   final challenge, and independent substantive review. The coordinator owns
   current-main composition with #434/#403, remote publication, required CI and
   normal squash integration. Recheck guards after their entrypoint changes.

## Focused proof and receipt

Use locked dependencies, two build jobs, and an isolated target. Retain command
logs with the exact candidate head/tree in the implementation handoff.

```sh
cargo test -p shipper-config --lib per_error --locked --offline
cargo test -p shipper-types --lib retry::tests --locked --offline
cargo test -p shipper-core --lib per_error_public_entrypoints --locked --offline
cargo test -p shipper-cli --test e2e_publish per_error --locked --offline
cargo test -p shipper-config --lib snapshot_default_toml_template --locked --offline
cargo test -p shipper-cli --test e2e_expanded config_init_content_snapshot --locked --offline
cargo fmt --all -- --check
cargo precommit
```

The API/CLI fixtures use fake Cargo and loopback registry spies. Do not treat a
sandbox listener denial as a production regression or a pass. Refusal tests must
fail if the shared validator is disabled; the positive execution controls must
fail if all retry configurations are refused. Preserve those negative-control
results separately from the candidate's passing proof.

## Stop conditions, rollback, and boundaries

Stop and repair if a configured class reaches an effect, empty/omitted settings
lose existing global behavior, a public entrypoint bypasses validation, or the
documentation again presents class execution as supported. Roll back by a normal
revert; preserve previously recorded execution evidence and #367 history.

No release credentials, registry publication, tags, signing, durable event/schema
extension, counter reset, broad retry/type refactor, or closure claim for separate
#402/#403 findings. The change refuses an inert configuration surface; release
and adoption evidence remain separately owned.
