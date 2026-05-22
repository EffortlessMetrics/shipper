# Product Overview

> Steering doc: orientation for contributors and AI assistants. For mission/vision/beliefs see [../MISSION.md](../MISSION.md). For the nine-competency scorecard and sequencing see [../ROADMAP.md](../ROADMAP.md). For per-release detail see [../CHANGELOG.md](../CHANGELOG.md).

## What Shipper is

Shipper is a publishing reliability layer for Rust workspaces. It wraps `cargo publish` with the workflow that production releases need: a deterministic plan, pre-flight proof, retry absorption, ambiguity reconciliation against registry truth, persistent state for recovery, and operator-grade evidence.

It is intentionally narrow. Cargo packages and uploads; Shipper owns everything between *"I want to release"* and *"all crates are live, here is the audit trail."* Version decisions, changelog generation, tags, and GitHub releases are deliberately out of scope — pair Shipper with [cargo-release](https://github.com/crate-ci/cargo-release) or [release-plz](https://github.com/MarcoIeni/release-plz) for those.

## Who it is for

Workspace maintainers who publish **multiple interdependent crates** as a coherent release, run that release **through CI**, treat publishing as **serious and audited**, and need recovery to be **mechanical rather than heroic**. Solo authors of single crates are already well-served by `cargo publish` directly; their workflow is fine. Ours is what happens when the workflow has consequences.

The full audience definition lives in [../MISSION.md](../MISSION.md#audience).

## Product shape

Shipper ships as three crates with distinct roles:

| Crate | Role | What it owns |
|---|---|---|
| **`shipper`** | Install face | The `shipper` binary (3-line forwarder), plus a curated library re-export of `shipper-core` for drivers that prefer the product name. This is the user-facing facade package and the stable `cargo install shipper --locked` handle. |
| **`shipper-cli`** | CLI adapter | `clap` parsing, subcommand dispatch, help text, progress rendering. Exposes `pub fn run() -> anyhow::Result<()>` as the embedding entry point. |
| **`shipper-core`** | Engine library | Plan, preflight, publish, resume, reconcile, rehearsal, remediate, state/events/receipts. **No CLI dependencies.** This is the stable embedding surface for IDP plugins, dashboards, and automation. |

All 13 workspace crates publish to crates.io on the same train. Engine internals (`shipper-types`, `shipper-registry`, `shipper-cargo-failure`, `shipper-retry`, `shipper-sparse-index`, `shipper-output-sanitizer`, `shipper-config`, `shipper-duration`, `shipper-encrypt`, `shipper-webhook`) live as peer library crates. See [structure.md](structure.md) for the full crate map and module layout.

## What Shipper does today

The nine competencies from [../ROADMAP.md](../ROADMAP.md) are all present in `main`:

- **Prove** — deterministic plan (`plan_id` = SHA256 of topo-sorted workspace), preflight (git cleanliness, registry reachability, dry-run, version existence, ownership), and a rehearsal registry pass with optional smoke-install before the live dispatch.
- **Survive** — per-step state persistence, workspace-aware locking, resume that reconciles before re-entering the retry loop, registry-aware backoff.
- **Reconcile** — ambiguous `cargo publish` outcomes are reconciled against registry truth (sparse index + API), not blind-retried. Cargo stdout is demoted to a fast-path hint.
- **Narrate** — structured retry/backoff events and live CLI narration so operators can see what the engine is waiting on and why.
- **Remediate** — receipt-driven dry-run artifacts, reverse-topological yank planning, fix-forward planning, and guarded fake-Cargo execution of reviewed plans. Live crates.io yank/fix-forward execution remains deliberately unpromoted.
- **Harden** — Trusted Publishing (OIDC) prerequisites and release auth evidence are first-class; the default remains planned/advisory until release evidence proves the short-lived-token path for the full crate set.
- **Consistency** — events-as-truth invariant enforced at end-of-run; drift is detected and reported.
- **Ergonomics** — the `shipper` install facade works end-to-end from a checkout and from public crates.io.
- **Integrate** — `shipper-core` is consumable as a library without pulling the CLI graph.

For per-capability status and sequencing, see [../ROADMAP.md](../ROADMAP.md). For what changed when, see [../CHANGELOG.md](../CHANGELOG.md).

## Non-goals

Shipper does **not** decide versions, generate changelogs, create git tags, create GitHub releases, manage crates.io team membership, or update dependencies. See [../MISSION.md](../MISSION.md#what-we-are-not) for the full list and the tools that do own each of those.

Shipper targets Rust → cargo-protocol registries. Alternative registries (Cloudsmith, kellnr, self-hosted) are in scope as they appear; other ecosystems are not.

## Compared to alternatives

| Tool | Use case | Relationship to Shipper |
|---|---|---|
| `cargo publish -p X` | Single crate upload | Shipper is the workflow wrapper around this primitive |
| `cargo publish --workspace` (Cargo 1.90+) | Multi-package upload | Same primitive; Shipper adds plan/preflight/state/reconcile/evidence on top |
| [cargo-release](https://github.com/crate-ci/cargo-release) | Version bump + tag + publish | Shipper covers the publish; cargo-release covers the versioning |
| [release-plz](https://github.com/MarcoIeni/release-plz) | PR-based automated releases | Drives cargo-release; can equally drive Shipper |
| `cargo workspaces publish` | Workspace publishing | Shipper adds preflight/state/resume/reconcile/evidence |

Shipper is what you reach for **after** version-decision and tag-creation are done, when the actual upload needs to be safe, resumable, and auditable.

## Where to go from here

| If you are… | Start here |
|---|---|
| A new user | [../README.md](../README.md) → [tutorials](tutorials) |
| An operator setting up CI | [release-runbook.md](release-runbook.md), [configuration.md](configuration.md) |
| An embedder | [`shipper-core`](../crates/shipper-core/README.md) → [structure.md](structure.md) |
| A contributor | [../MISSION.md](../MISSION.md) → [../ROADMAP.md](../ROADMAP.md) → [../CONTRIBUTING.md](../CONTRIBUTING.md) |
| Auditing receipts or events | [INVARIANTS.md](INVARIANTS.md) |
| An AI assistant | [../CLAUDE.md](../CLAUDE.md) or [../GEMINI.md](../GEMINI.md) |
