# Release preparation and evidence

Shipper uses two repositories with different authority:

| Repository | Authority |
| --- | --- |
| `EffortlessMetrics/shipper-swarm` | active development, changelog preparation, candidate proof, and source freeze |
| `EffortlessMetrics/shipper` | release rehearsal, tags, crates.io publication, GitHub Releases, signing, and release credentials |

A green swarm commit is not publication authorization. A release becomes tag-eligible only after the exact frozen swarm candidate has been promoted to `shipper` with a non-fast-forward merge commit and the merged release-authority SHA has completed its own rehearsal and binary proof.

## Canonical documents

- [Release operator runbook](../release-runbook.md) — the ordered procedure from changelog preparation through post-release backfill.
- [Release preparation checklist](release-preparation-checklist.md) — copy this for each candidate and record exact SHAs, trees, workflow runs, artifacts, and stop decisions.
- [Current 0.5.0 preparation control record](0.5.0-preparation.md) — live blockers, superseded publication identity, and the fields that must be populated by the next fresh candidate.
- [0.5.0 readiness record](0.5.0-readiness.md) — retained product, compatibility, semver, binary, and historical candidate evidence for the 0.5 line.
- [Swarm operation policy](../status/SWARM_OPERATION.md) — development/release authority, merge model, queue freeze, and source-backfill rules.
- [Swarm sync policy](../status/SWARM_SYNC.md) — history-preserving promotion and backfill mechanics.
- [Changie workflow](../how-to/manage-changelog-fragments.md) — local fragment intake, the retained pre-Changie baseline, batching, and round-trip proof.

- [0.5.0 change ledger](0.5.0-change-ledger.md) — exact source inventory,
  canonical release-story dispositions, compatibility coverage, and editorial
  claim boundaries for the 0.5.0 changelog pass.
- [0.5.0 release notes](../../RELEASE_NOTES_v0.5.0.md) — concise operator
  narrative and pre-publication upgrade boundary.
- [0.5.0 migration guide](0.5.0-migration.md) — Rust source, durable artifact,
  configuration, CLI/CI, recovery, and release-maintainer changes.

## Evidence identity rule

Every candidate record must distinguish these identities:

1. **Swarm candidate SHA** — the exact reviewed `shipper-swarm/main` commit whose source and changelog are frozen.
2. **Swarm candidate tree** — used to prove the promotion merge did not alter the candidate tree.
3. **Promotion merge SHA** — the non-fast-forward merge commit in `EffortlessMetrics/shipper`.
4. **Release-authority approved SHA** — the exact `shipper/main` commit after any separately reviewed release-authority-only changes.
5. **Tag SHA** — must equal the approved release-authority SHA.
6. **Publish run and final state artifact** — retained evidence for the irreversible train.

Do not reuse a green check from a different SHA. Evidence from an earlier candidate may support an unchanged claim only when the new candidate record identifies the earlier evidence, records the exact diff boundary, and explains why that diff cannot affect the claim. The required candidate-head Rust and release-authority rehearsal gates are always rerun.

## Current 0.5 posture

The previously recorded 0.5 candidate, promotion, semver proof, and binary matrix remain useful evidence. Subsequent release-preparation changes in `shipper-swarm` reopen the exact-candidate identity, however. The live [0.5.0 preparation record](0.5.0-preparation.md) therefore marks publication authorization `NO` until the focused preparation PRs merge, a new swarm SHA/tree is frozen, a fresh history-preserving promotion lands, and the release-authority gates pass on the new approved SHA.
