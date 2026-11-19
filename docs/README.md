# Shipper Documentation

Organized by reader purpose ([Diátaxis](https://diataxis.fr/)). Pick the column that matches what you need right now.

| Need | Go to |
|---|---|
| **Learn** by doing a task end-to-end | [Tutorials](#tutorials) |
| **Solve** a specific problem you already understand | [How-to guides](#how-to-guides) |
| **Look up** exact command, flag, or schema | [Reference](#reference) |
| **Understand** why Shipper works the way it does | [Explanation](#explanation) |

---

## Tutorials

Step-by-step learning paths. Start here if you've never used Shipper before.

- [First publish — from a toy workspace](tutorials/first-publish.md)
- [Getting to release confidence in five minutes](tutorials/getting-started-5-minutes.md)
- [Recover from an interrupted release](tutorials/recover-from-interruption.md)

## How-to guides

Task-oriented recipes. Each solves one focused problem.

- [Run a release in GitHub Actions](how-to/run-in-github-actions.md)
- [Publish missing workspace crates](how-to/publish-missing-workspace-crates.md)
- [Inspect state, events, and receipts](how-to/inspect-state-and-receipts.md) — post-hoc inspection ("what happened")
- [Inspect a stalled or interrupted run](how-to/inspect-a-stalled-run.md) — live triage ("is it alive?")
- [Run the Recover rehearsal](how-to/run-recover-rehearsal.md) — once-per-RC proof that interrupted releases resume cleanly
- [Rehearse against an alternate registry](how-to/rehearse-against-an-alt-registry.md) — Prove tier 2 walkthrough with kellnr example (#97)
- [Remediate a compromised release](how-to/remediate-a-compromised-release.md) — yank + fix-forward walkthrough (#98)
- [Manage Changie fragments locally](how-to/manage-changelog-fragments.md) — staged-index authoring, retained history, batching, merge, and round-trip proof
- [Migrate `shipper` to `shipper-swarm` (runbook)](how-to/shipper-swarm-migration-runbook.md) — completed development-authority cutover and CI-routing history

## Release preparation

- [Release preparation and evidence index](release/README.md) — authority, exact-identity rules, and current release-line posture
- [0.5.0 release notes](../RELEASE_NOTES_v0.5.0.md) — reviewed GitHub Release publication input; the file alone is not publication evidence
- [0.5.0 migration guide](release/0.5.0-migration.md) — source, config, artifact, command, and recovery changes for the candidate
- [0.5.0 change ledger](release/0.5.0-change-ledger.md) — complete editorial source inventory and compatibility dispositions
- [Release operator runbook](release-runbook.md) — ordered candidate, promotion, rehearsal, publication, resume, and backfill procedure
- [Release preparation checklist](release/release-preparation-checklist.md) — copy per candidate and record exact SHAs, trees, workflow runs, artifacts, and authorization
- [0.5.0 release-candidate readiness](release/0.5.0-readiness.md) — retained product and historical candidate evidence for the 0.5 line; a later source change requires a fresh candidate identity before publication

## Reference

Exhaustive, precise, stable specs.

- [CLI reference](reference/cli.md) (canonical source: `shipper --help` / `shipper <cmd> --help`)
- [State files cheat sheet](reference/state-files.md) — `.shipper/` file roles, authority order, jq recipes
- [`.shipper.toml` configuration](configuration.md)
- [Preflight checks](preflight.md)
- [Readiness verification](readiness.md)
- [Failure modes and typed recovery decisions](failure-modes.md) — retry classes, exit vocabulary, durable evidence, and fail-closed recovery
- [Support tiers](status/SUPPORT_TIERS.md) — claim maturity, release-lifecycle qualifiers, and proof ownership
- [JSON evidence contracts](specs/SHIPPER-SPEC-0004-json-evidence-contracts.md) — canonical command schema and compatibility map
- [0.5.0 scheduler conformance ledger](../plans/0.5.0-scheduler-conformance.md)

## Explanation

Design decisions and reasoning. Read these to understand *why* things are the way they are.

- [Why Shipper exists](explanation/why-shipper.md)
- [Understanding `finishability` (especially `not_proven`)](explanation/finishability.md)
- [Architecture](architecture.md)
- [Events-as-truth invariant](INVARIANTS.md)
- [Product overview](product.md)
- [Repository structure](structure.md)
- [Tech stack](tech.md)
- [Tool substrate standard](ci/tool-substrate.md)

## Root-level orientation

The following live at the repo root because they carry repo-wide authority:

- [MISSION.md](../MISSION.md) — mission, vision, audience, beliefs
- [ROADMAP.md](../ROADMAP.md) — five pillars, nine-competency scorecard, now/next/later
- [README.md](../README.md) — product README
- [CLAUDE.md](../CLAUDE.md) / [GEMINI.md](../GEMINI.md) / [AGENTS.md](../AGENTS.md) — AI-assistant orientation
- [CONTRIBUTING.md](../CONTRIBUTING.md) — contribution guide
- [SECURITY.md](../SECURITY.md) — security policy
- [CHANGELOG.md](../CHANGELOG.md) — release history

## Repository maintenance

Internal inventories and upkeep docs for contributor and assistant context.

- [Status docs](status/README.md) — claim maturity, release lifecycle, support tiers, and swarm operation policy
- [Crate local-doc coverage](reference/crate-coverage.md) — maintenance matrix for crate `README.md`, `CLAUDE.md`, and `AGENTS.md` files
