# Shipper Roadmap

> See [MISSION.md](MISSION.md) for the mission, vision, audience, and beliefs that produce the priorities below.

## Where we are

**v0.4.0 shipped 2026-05-20.** Thirteen public crates are live on crates.io, and `cargo install shipper --locked` is the stable install path. The 0.4.0 release line made Shipper an idempotent, evidence-backed workspace publisher: it plans missing `name@version` pairs, preflights proof and pacing, reconciles ambiguous Cargo outcomes against registry truth, resumes from durable state, and records release evidence.

The post-release retrospective produced a product thesis organized around nine competencies. This document is structured around them. Each competency has a tracking issue (#100–#108); the master roadmap is **#109**.

## Five existential pillars (the safety claim)

Cargo 1.90 stabilized multi-package workspace publishing. "Publish several crates at once" is no longer a differentiator. Shipper is only worth existing if it owns five guarantees Cargo still does not give you — together they are the **release-closure system** that the engine is moving toward:

| Pillar | Question it answers | Status |
|---|---|---|
| **Prove** | Can I show this release is safe *before* the irreversible step? | Stable base — plan/preflight JSON, dry-run, ownership where possible, registry pacing, and alternate-registry rehearsal surfaces exist; stronger schema/provenance work remains follow-up |
| **Dispatch** | Is the publish executed in a registry-aware, paced way? | Partial — crates.io first-publish backoff, `Retry-After` retry floors, and preflight registry-pacing estimates exist; alternative registry profiles pending ([#94](https://github.com/EffortlessMetrics/shipper/issues/94) / [#106](https://github.com/EffortlessMetrics/shipper/issues/106)) |
| **Reconcile** | When the result is ambiguous, do I check registry truth before retrying? | Implemented — ambiguous exits reconcile to Published / NotPublished / StillUnknown before retry ([#99](https://github.com/EffortlessMetrics/shipper/issues/99) / [#102](https://github.com/EffortlessMetrics/shipper/issues/102)) |
| **Recover** | If the runner dies mid-train, can I converge from durable state without losing or duplicating work? | Stable/internal proof — synthetic resume and live-runner artifact handoff are proven against fake Cargo/mock registry; live crates.io interruption remains a release-candidate procedure |
| **Remediate** | If a partial release goes bad, can I contain or fix-forward it mechanically? | Bounded — receipt-driven planning, dry-run artifacts, and guarded fake-Cargo execution exist; live crates.io yank/fix-forward execution remains advisory |

These five are the existential pillars: a publishing tool that doesn't own them is a publishing tool that asks too much trust from the operator. Today Shipper has closed the first Reconcile implementation path and remains mid-flight on Prove, Dispatch, Recover, and Remediate. The remaining work turns "useful release executor" into "trustworthy release-closure system."

## Nine competencies (the full scorecard)

The five pillars cover the safety story. Four more competencies — narrate, harden, integrate, ergonomics — make the tool legible, securable, embeddable, and approachable. Together they form the full scorecard each tracking issue maps to:

| # | Competency | Definition | Status | Issue |
|---|---|---|---|---|
| 1 | **Prove** | Establish before the irreversible step that the publish can succeed | Partial | [#100](https://github.com/EffortlessMetrics/shipper/issues/100) |
| 2 | **Survive** | Recover from interruption without losing or duplicating work | Partial | [#101](https://github.com/EffortlessMetrics/shipper/issues/101) |
| 3 | **Reconcile** | Close ambiguous outcomes against registry truth | Implemented | [#102](https://github.com/EffortlessMetrics/shipper/issues/102) |
| 4 | **Narrate** | Tell the operator what's happening live, not just after the fact | Partial | [#103](https://github.com/EffortlessMetrics/shipper/issues/103) |
| 5 | **Remediate** | Mechanically recover from bad partial outcomes (yank, fix-forward) | Bounded | [#104](https://github.com/EffortlessMetrics/shipper/issues/104) |
| 6 | **Harden** | Default to safe auth posture; minimize long-lived secret blast radius | Partial | [#105](https://github.com/EffortlessMetrics/shipper/issues/105) |
| 7 | **Profile** | Encode what we know about each registry (rate limits, regimes) | Partial | [#106](https://github.com/EffortlessMetrics/shipper/issues/106) |
| 8 | **Integrate** | Consumable from IDP platforms and CI orchestration tooling | Partial | [#107](https://github.com/EffortlessMetrics/shipper/issues/107) |
| 9 | **Ergonomics** | First-impression friction is low; defaults are sensible | Partial | [#108](https://github.com/EffortlessMetrics/shipper/issues/108) |

**Master tracking issue: [#109](https://github.com/EffortlessMetrics/shipper/issues/109)**

The biggest single gap used to be **#3 Reconcile**: when `cargo publish` returns ambiguously (it uploads first, then polls the index, and the poll can time out without affecting the upload), Shipper could retry blindly. Shipper now reconciles ambiguous outcomes against registry truth before retry; remaining safety work is concentrated in registry-aware pacing, live interruption proof, and remediation.

## Design principles

These guide all Shipper development.

### Reliability over speed
Default behaviors verify, log, and provide evidence. Faster paths are explicit opt-ins. The default publish policy (`safe`) includes all verification.

### Determinism
Publish order is reproducible. Plan IDs are SHA256 of the workspace plan and stable across environments. The same workspace state always produces the same `plan_id`.

### Events are truth, state is a projection
Per [docs/INVARIANTS.md](docs/INVARIANTS.md): `events.jsonl` is authoritative and append-only. `state.json` is a projection over events for resume convenience. `receipt.json` is a summary derived from events at end-of-run. The relationship is contractual; see [#93](https://github.com/EffortlessMetrics/shipper/issues/93) for enforcement.

### Engine is library; CLI is thin
All release behavior lives in `crates/shipper-core`. `crates/shipper-cli` parses args and calls into the engine. `crates/shipper` is the install facade and curated product-name re-export. Other frontends (IDP plugins, dashboards, automation) consume `shipper-core` or the curated facade directly.

### Forbid unsafe; respect MSRV
`unsafe_code = "forbid"` workspace-wide. Edition 2024, MSRV 1.95.

## Now / Next / Later

Sequencing follows the master roadmap ([#109](https://github.com/EffortlessMetrics/shipper/issues/109)).

### Now — after v0.4.0
1. **[#105](https://github.com/EffortlessMetrics/shipper/issues/105) Harden** — keep Trusted Publishing default planned/advisory until release evidence proves the short-lived-token path for the full crate set. Current 0.4.0 evidence records explicit fallback-secret use.
2. **[#103](https://github.com/EffortlessMetrics/shipper/issues/103) Narrate** — continue improving live wait/retry/readiness visibility and status/watch surfaces so long registry waits never look hung.
3. **[#101](https://github.com/EffortlessMetrics/shipper/issues/101) Survive** — turn events/state/receipt consistency and state rebuild into boring operator recovery surfaces, building on the synthetic and live-runner fake-Cargo proofs.
4. **[#107](https://github.com/EffortlessMetrics/shipper/issues/107) Integrate** — make the stable JSON envelopes, receipts, and `.shipper/` packet easy for CI, IDPs, and agents to consume.
5. **[#104](https://github.com/EffortlessMetrics/shipper/issues/104) Remediate** — promote only the proof-backed remediation surfaces: dry-run artifacts and guarded fake-Cargo execution today; live crates.io yank/fix-forward execution only after deliberate evidence.

### Next
6. **[#100](https://github.com/EffortlessMetrics/shipper/issues/100) Prove tier 2** — keep strengthening alternate-registry rehearsal and smoke-install proof as an explicit tier above local dry-run.
7. **[#106](https://github.com/EffortlessMetrics/shipper/issues/106) Profile** — extend beyond the proof-backed crates.io profile when another registry has evidence-backed pacing semantics.

### Later — once the engine is closure-complete
8. **Advanced integrations** — IDP plugin examples (Backstage / Port / Cortex), HTTP query API, webhook reliability semantics, and richer library consumer guides. Best done after the engine offers a stable closure story to integrate against.

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
2. Whether it closes a proof or operator-confidence gap in the current release-closure line
3. Maintenance burden vs value

To contribute:
1. Pick an issue from #100–#109 or one of the child issues #90–#99
2. Comment to claim
3. Open a draft PR with tests + documentation per [CONTRIBUTING.md](CONTRIBUTING.md)

## Version history

| Version | Date | Theme |
|---|---|---|
| v0.4.0 | 2026-05-20 | Stable release-closure line; idempotent workspace publish, JSON evidence envelopes, registry-truth reconciliation, resume proof, auth evidence, and bounded remediation surfaces |
| v0.3.0-rc.1 | 2026-04-16 | First crates.io publish; 12 crates live; deterministic plan, retry absorption (41 retries), evidence trail proven under real rate limits |
| v0.2.0 | 2026-02-14 | Evidence + verification (event log, receipts, readiness checks, publish policies) |
| v0.1.0 | — | Initial release |
