# Shipper Roadmap

> See [MISSION.md](MISSION.md) for the mission, vision, audience, and beliefs that produce the priorities below.

## Where we are

**v0.3.0-rc.1 shipped 2026-04-16.** Twelve crates went live on crates.io: `shipper`, `shipper-cli`, `shipper-config`, `shipper-types`, `shipper-registry`, `shipper-duration`, `shipper-retry`, `shipper-encrypt`, `shipper-output-sanitizer`, `shipper-cargo-failure`, `shipper-sparse-index`, `shipper-webhook`. The publish train was driven by Shipper itself — first real-world dogfooding under crates.io rate limits, with 41 retries silently absorbed across a 69-minute run.

The post-release retrospective produced a product thesis organized around nine competencies. This document is structured around them. Each competency has a tracking issue (#100–#108); the master roadmap is **#109**.

## Five existential pillars (the safety claim)

Cargo 1.90 stabilized multi-package workspace publishing. "Publish several crates at once" is no longer a differentiator. Shipper is only worth existing if it owns five guarantees Cargo still does not give you — together they are the **release-closure system** that the engine is moving toward:

| Pillar | Question it answers | Status |
|---|---|---|
| **Prove** | Can I show this release is safe *before* the irreversible step? | Partial — workspace dry-run + ownership; rehearsal-registry pending ([#97](https://github.com/EffortlessMetrics/shipper/issues/97)) |
| **Dispatch** | Is the publish executed in a registry-aware, paced way? | Partial — generic exponential backoff; documented-constraint + Retry-After layering pending ([#94](https://github.com/EffortlessMetrics/shipper/issues/94)) |
| **Reconcile** | When the result is ambiguous, do I check registry truth before retrying? | **Missing** ([#99](https://github.com/EffortlessMetrics/shipper/issues/99) / [#102](https://github.com/EffortlessMetrics/shipper/issues/102)) — biggest open gap |
| **Recover** | If the runner dies mid-train, can I converge from durable state without losing or duplicating work? | Partial — implemented; verification under real interruption pending ([#90](https://github.com/EffortlessMetrics/shipper/issues/90)) |
| **Remediate** | If a partial release goes bad, can I contain or fix-forward it mechanically? | **Missing** ([#98](https://github.com/EffortlessMetrics/shipper/issues/98) / [#104](https://github.com/EffortlessMetrics/shipper/issues/104)) |

These five are the existential pillars: a publishing tool that doesn't own them is a publishing tool that asks too much trust from the operator. Today Shipper is two-thirds of the way through pillars 1, 2, and 4; pillars 3 and 5 are the work that turns "useful release executor" into "trustworthy release-closure system."

## Nine competencies (the full scorecard)

The five pillars cover the safety story. Four more competencies — narrate, harden, integrate, ergonomics — make the tool legible, securable, embeddable, and approachable. Together they form the full scorecard each tracking issue maps to:

| # | Competency | Definition | Status | Issue |
|---|---|---|---|---|
| 1 | **Prove** | Establish before the irreversible step that the publish can succeed | Partial | [#100](https://github.com/EffortlessMetrics/shipper/issues/100) |
| 2 | **Survive** | Recover from interruption without losing or duplicating work | Partial | [#101](https://github.com/EffortlessMetrics/shipper/issues/101) |
| 3 | **Reconcile** | Close ambiguous outcomes against registry truth | **Missing** | [#102](https://github.com/EffortlessMetrics/shipper/issues/102) |
| 4 | **Narrate** | Tell the operator what's happening live, not just after the fact | Partial | [#103](https://github.com/EffortlessMetrics/shipper/issues/103) |
| 5 | **Remediate** | Mechanically recover from bad partial outcomes (yank, fix-forward) | **Missing** | [#104](https://github.com/EffortlessMetrics/shipper/issues/104) |
| 6 | **Harden** | Default to safe auth posture; minimize long-lived secret blast radius | Partial | [#105](https://github.com/EffortlessMetrics/shipper/issues/105) |
| 7 | **Profile** | Encode what we know about each registry (rate limits, regimes) | Partial | [#106](https://github.com/EffortlessMetrics/shipper/issues/106) |
| 8 | **Integrate** | Consumable from IDP platforms and CI orchestration tooling | Partial | [#107](https://github.com/EffortlessMetrics/shipper/issues/107) |
| 9 | **Ergonomics** | First-impression friction is low; defaults are sensible | Partial | [#108](https://github.com/EffortlessMetrics/shipper/issues/108) |

**Master tracking issue: [#109](https://github.com/EffortlessMetrics/shipper/issues/109)**

The biggest single gap is **#3 Reconcile**: when `cargo publish` returns ambiguously (it uploads first, then polls the index, and the poll can time out without affecting the upload), Shipper today retries blindly. Without ambiguity reconciliation against registry truth, the safety pitch has a hole at the worst possible spot.

## Design principles

These guide all Shipper development.

### Reliability over speed
Default behaviors verify, log, and provide evidence. Faster paths are explicit opt-ins. The default publish policy (`safe`) includes all verification.

### Determinism
Publish order is reproducible. Plan IDs are SHA256 of the workspace plan and stable across environments. The same workspace state always produces the same `plan_id`.

### Events are truth, state is a projection
Per [docs/INVARIANTS.md](docs/INVARIANTS.md): `events.jsonl` is authoritative and append-only. `state.json` is a projection over events for resume convenience. `receipt.json` is a summary derived from events at end-of-run. The relationship is contractual; see [#93](https://github.com/EffortlessMetrics/shipper/issues/93) for enforcement.

### Engine is library; CLI is thin
All domain logic lives in `crates/shipper`. `crates/shipper-cli` parses args and calls into the library. Other frontends (IDP plugins, dashboards, automation) consume the library directly.

### Forbid unsafe; respect MSRV
`unsafe_code = "forbid"` workspace-wide. Edition 2024, MSRV 1.92.

## Now / Next / Later

Sequencing follows the master roadmap ([#109](https://github.com/EffortlessMetrics/shipper/issues/109)).

### Now — gates v0.3.0 stable
1. **[#102](https://github.com/EffortlessMetrics/shipper/issues/102) Reconcile** — implement the ambiguous-outcome state machine ([#99](https://github.com/EffortlessMetrics/shipper/issues/99)). Without this, Shipper cannot truthfully claim to be safer than naive `cargo publish`.
2. **[#105](https://github.com/EffortlessMetrics/shipper/issues/105) Harden** — make Trusted Publishing the default ([#96](https://github.com/EffortlessMetrics/shipper/issues/96)). Most of the wiring exists.
3. **[#103](https://github.com/EffortlessMetrics/shipper/issues/103) Narrate** — surface retry/backoff state ([#91](https://github.com/EffortlessMetrics/shipper/issues/91)). Operators currently fly blind for ~60 minutes during rate-limited publishes.
4. **[#101](https://github.com/EffortlessMetrics/shipper/issues/101) Survive** — execute the rehearsal procedure ([#90](https://github.com/EffortlessMetrics/shipper/issues/90)) and document the events-as-truth invariant ([#93](https://github.com/EffortlessMetrics/shipper/issues/93)). Resume is currently unverified under real interruption.
5. **[#108](https://github.com/EffortlessMetrics/shipper/issues/108) Ergonomics** — `cargo install shipper` should work ([#95](https://github.com/EffortlessMetrics/shipper/issues/95)).

### Next — past stable
6. **[#106](https://github.com/EffortlessMetrics/shipper/issues/106) Profile** — registry-aware backoff layered on the regime tag preflight already detects ([#94](https://github.com/EffortlessMetrics/shipper/issues/94)). Preflight discovers `is_new_crate`; publish discards it. Threading the tag through is the entire mechanism.
7. **[#104](https://github.com/EffortlessMetrics/shipper/issues/104) Remediate** — receipt-driven yank/fix-forward for compromised releases ([#98](https://github.com/EffortlessMetrics/shipper/issues/98)).
8. **[#100](https://github.com/EffortlessMetrics/shipper/issues/100) Prove tier 2** — rehearsal registry as the next preflight strength ([#97](https://github.com/EffortlessMetrics/shipper/issues/97)). Promotes preflight from "we believe" to "we proved against a registry-shaped target".

### Later — once the engine is closure-complete
9. **[#107](https://github.com/EffortlessMetrics/shipper/issues/107) Integrate** — IDP plugin examples (Backstage / Port / Cortex), HTTP query API, webhook reliability semantics, library consumer guide. Best done after the engine offers a stable closure story to integrate against.

## Explicit non-goals

Shipper does NOT plan to support:

| Feature | Alternative |
|---|---|
| Version bumping | [cargo-release](https://github.com/crate-ci/cargo-release) |
| Changelog generation | [release-plz](https://github.com/MarcoIeni/release-plz) |
| Git tag creation | cargo-release |
| GitHub release creation | `gh` CLI or GitHub Actions |
| crates.io team management | `cargo owner` |
| Dependency updates | cargo's built-in commands |

**Shipper focuses on reliable publishing, not release orchestration.**

## Contributing

How features are prioritized:
1. Fit with the nine-competency thesis
2. Whether it closes a gap blocking v0.3.0 stable
3. Maintenance burden vs value

To contribute:
1. Pick an issue from #100–#109 or one of the child issues #90–#99
2. Comment to claim
3. Open a draft PR with tests + documentation per [CONTRIBUTING.md](CONTRIBUTING.md)

## Version history

| Version | Date | Theme |
|---|---|---|
| v0.3.0-rc.1 | 2026-04-16 | First crates.io publish; 12 crates live; deterministic plan, retry absorption (41 retries), evidence trail proven under real rate limits |
| v0.2.0 | 2026-02-14 | Evidence + verification (event log, receipts, readiness checks, publish policies) |
| v0.1.0 | — | Initial release |
