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
- [Recover from an interrupted release](tutorials/recover-from-interruption.md)

## How-to guides

Task-oriented recipes. Each solves one focused problem.

- [Run a release in GitHub Actions](how-to/run-in-github-actions.md)
- [Inspect state, events, and receipts](how-to/inspect-state-and-receipts.md)

Operator runbook (promotion to how-to pending): [release-runbook.md](release-runbook.md)

## Reference

Exhaustive, precise, stable specs.

- [CLI reference](reference/cli.md) (canonical source: `shipper --help` / `shipper <cmd> --help`)
- [`.shipper.toml` configuration](configuration.md)
- [Preflight checks](preflight.md)
- [Readiness verification](readiness.md)
- [Failure modes](failure-modes.md)

## Explanation

Design decisions and reasoning. Read these to understand *why* things are the way they are.

- [Why Shipper exists](explanation/why-shipper.md)
- [Architecture](architecture.md)
- [Events-as-truth invariant](INVARIANTS.md)
- [Product overview](product.md)
- [Repository structure](structure.md)
- [Tech stack](tech.md)

## Root-level orientation

The following live at the repo root because they carry repo-wide authority:

- [MISSION.md](../MISSION.md) — mission, vision, audience, beliefs
- [ROADMAP.md](../ROADMAP.md) — five pillars, nine-competency scorecard, now/next/later
- [README.md](../README.md) — product README
- [CLAUDE.md](../CLAUDE.md) / [GEMINI.md](../GEMINI.md) — AI-assistant orientation
- [CONTRIBUTING.md](../CONTRIBUTING.md) — contribution guide
- [SECURITY.md](../SECURITY.md) — security policy
- [CHANGELOG.md](../CHANGELOG.md) — release history
