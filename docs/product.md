# Product Overview

> Quick orientation for contributors and AI assistants. For mission/vision/beliefs see [../MISSION.md](../MISSION.md). For sequencing see [../ROADMAP.md](../ROADMAP.md).

## What Shipper is

Shipper is a publishing reliability layer for Rust workspaces. It wraps `cargo publish` with the workflow that production releases need: deterministic planning, pre-flight proof, retry absorption, ambiguity reconciliation against registry truth, persistent state, and operator-grade observability.

It is intentionally narrow. Cargo packages and uploads; Shipper handles everything between *"I want to release"* and *"all crates are live, here's the audit trail."*

## Who it is for

See [../MISSION.md](../MISSION.md) for the full audience definition. Briefly: workspace maintainers who publish multiple interdependent crates as coherent releases through CI, and need recovery to be mechanical rather than heroic.

## What's shipped (v0.3.0-rc.1)

| Capability | Status |
|---|---|
| Deterministic publish planning (topo-sort + plan_id) | Production |
| Workspace dry-run preflight | Production |
| Retry/backoff with HTTP 429 handling | Production |
| Per-step state persistence and resume | Production (resume verification pending — see [#90](https://github.com/EffortlessMetrics/shipper/issues/90)) |
| Audit receipts with evidence | Production |
| Multi-registry orchestration | Production |
| Sparse-index caching | Production |
| Workspace-aware locking | Production |
| Shell completions, doctor diagnostics | Production |
| OIDC trusted publishing detection | Detection only ([#96](https://github.com/EffortlessMetrics/shipper/issues/96)) |
| Ambiguity reconciliation | Missing ([#99](https://github.com/EffortlessMetrics/shipper/issues/99) / [#102](https://github.com/EffortlessMetrics/shipper/issues/102)) |
| Live retry visibility for operators | Missing ([#91](https://github.com/EffortlessMetrics/shipper/issues/91) / [#103](https://github.com/EffortlessMetrics/shipper/issues/103)) |
| Yank planning / fix-forward | Missing ([#98](https://github.com/EffortlessMetrics/shipper/issues/98) / [#104](https://github.com/EffortlessMetrics/shipper/issues/104)) |

See [../ROADMAP.md](../ROADMAP.md) for the nine-competency status table and sequencing.

## Compared to alternatives

| Tool | Use case | Relationship |
|---|---|---|
| `cargo publish -p X` | Single crate | Shipper is the workflow wrapper |
| [cargo-release](https://github.com/crate-ci/cargo-release) | Version bump + tag + publish | Shipper covers the publish; cargo-release covers the versioning |
| [release-plz](https://github.com/MarcoIeni/release-plz) | PR-based automated releases | Drives cargo-release; can drive Shipper |
| `cargo workspaces publish` | Workspace publishing | Shipper adds preflight/state/resume/retry/evidence |
| Cargo 1.90 `cargo publish --workspace` | Multi-package publish | Same primitive — Shipper adds the reliability layer on top |

Shipper is what you reach for **after** version-decision and tag-creation are done, when the actual upload needs to be safe and recoverable.

## Non-goals

See [../MISSION.md](../MISSION.md) "What we are not."
