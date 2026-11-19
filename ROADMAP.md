# Shipper Roadmap

> See [MISSION.md](MISSION.md) for the mission, vision, audience, and beliefs that produce the priorities below.

## Where we are

**v0.4.0 shipped 2026-05-20.** Thirteen public crates are live on crates.io, and `cargo install shipper --locked` is the stable install path. The 0.4.0 release line made Shipper an idempotent, evidence-backed workspace publisher: it plans missing `name@version` pairs, preflights proof and pacing, reconciles ambiguous Cargo outcomes against registry truth, resumes from durable state, and records release evidence.

The post-v0.3.0 retrospective organized the product thesis around nine competencies. Issues #100–#109 are the historical implementation ledger for that model; completed competencies are no longer open planning queues. This document is the current roadmap.

The [build-readiness handoff](plans/swarm/build-readiness.md) connects the
2026-09-08 source and issue audit to bounded implementation work, proof, and
cleanup. It preserves the current release queue and the explicitly deferred
post-0.5 lanes; refresh its linked issues before selecting work.

## Adoption outcome — by 2026-12-31

The product goal is for Steven to use Shipper in an actual release workflow
before the end of 2026. Completion requires an observed consumer outcome:

- A pinned, installable Shipper performs the package plan and preflight for a
  real candidate, with actionable human output and matching machine evidence.
- An interrupted multi-crate rehearsal continues from transferred evidence,
  without duplicate upload or a false whole-plan success result.
- The next selected, explicitly approved release uses Shipper for its package
  transaction, retains its receipt, and verifies the published packages.
- A subsequent inspection or rerun answers what completed and what remains
  through supported commands, without reconstructing the answer from logs.

The first release consumer is Shipper's own thirteen-crate train through the
existing release-authority workflow. The external adoption target is RIPR's
package workflow under [shipper#455](https://github.com/EffortlessMetrics/shipper/issues/455).
RIPR starts with read-only plan/preflight and historical replay; required
transaction cutover follows the existing identity, parity, and authority
prerequisites. RIPR currently has one publishable package, so its adoption alone
does not establish multi-crate recovery.

The [dated delivery checkpoints](plans/swarm/build-readiness.md#year-end-use-checkpoints--2026-10-26)
connect those outcomes to the existing issue owners. Source fixes, a public
release, and actual consumer adoption each need their own evidence. The date
does not waive release review or grant publication authority.

## 0.5.0 release-line status

The 0.5.0 line is the capstone for the current execution and security work. `shipper-swarm` is the development and proof surface; the separate `EffortlessMetrics/shipper` repository remains the release authority for tags, crates.io publication, GitHub Release creation, and final release evidence.

The stabilized boundaries are one per-package execution authority shared by both schedulers, event-first transitions with rebuild evidence, one readiness polling kernel, validated registry destinations and redirects, context-aware authorization redaction, versioned KDF compatibility, and blank-value-safe OIDC diagnostics. A green swarm branch is not itself a release: promotion still requires one exact candidate SHA and release-authority evidence. Release credentials must not move into this repository.

The active release sequence is explicit:

1. freeze and review one swarm candidate;
2. promote that history to the source-authority repository;
3. rehearse the exact promoted SHA and record GO or NO-GO;
4. publish only after explicit authorization;
5. verify public artifacts and backfill the final authority history into swarm.

### Development handoff — 2026-11-02

The October repair train is merged in development, but its implementation,
exact-candidate qualification, release-authority adoption, public availability,
and actual consumer use are separate outcomes. The live queue and evidence
remain in [swarm #255](https://github.com/EffortlessMetrics/shipper-swarm/issues/255);
this dated handoff neither freezes a source nor authorizes publication.

Effective-registry planning, selected-workspace receipt identity, truthful
whole-plan completion, configuration preservation, retained-state admission,
and live retry narration have landed. Preserve their focused regression
proof instead of reopening the original implementation tasks. The selected
[#362](https://github.com/EffortlessMetrics/shipper-swarm/issues/362) correction
rejects unsupported nonempty per-error retry settings through
[PR #439](https://github.com/EffortlessMetrics/shipper-swarm/pull/439);
superseded PR #367's richer runtime is not a remaining release prerequisite.

The remaining work has distinct acceptance boundaries:

| Owner | Next accepted outcome |
|---|---|
| [#412](https://github.com/EffortlessMetrics/shipper-swarm/issues/412) | Preserve Linux cleanup proof, qualify the supervised macOS path, and resolve or explicitly disposition the Windows implementation gap. Windows still uses direct-child supervision and reader joins that can wait for inherited EOF; this is not merely a missing native test run. Archive startup does not prove cleanup. |
| [#417](https://github.com/EffortlessMetrics/shipper-swarm/issues/417) | Run each actual optimized archive's extracted executable on its matching native target and retain source/digest/version-bound receipts. The source-context defect in [#454](https://github.com/EffortlessMetrics/shipper-swarm/issues/454) needs its own reviewed correction and #255 disposition; a proposed repair is not landed proof. |
| [#418](https://github.com/EffortlessMetrics/shipper-swarm/issues/418) | Complete actual cross-run publish → resume → resume using each predecessor's produced artifact, then consume the mechanism in the release authority. Local repeated-continuation and same-run/two-job transport proof remain useful but do not establish this outcome or abrupt host-loss recovery. |
| [#370](https://github.com/EffortlessMetrics/shipper-swarm/issues/370) / [#255](https://github.com/EffortlessMetrics/shipper-swarm/issues/255) | Run the existing broad hosted route on the final accepted source and finish the complete candidate matrix. Keep self-hosted capacity diagnosis separate; a Linux route does not replace native archive or cross-run proof. |
| [shipper #475–#479](https://github.com/EffortlessMetrics/shipper/issues/475) | Promote qualified history, finalize approved identity/date, rehearse and record GO/NO-GO, publish only after explicit GO, verify public artifacts, and backfill history. Successful resume still needs exact-version/public-install verification and release finalization. |

Resolve accepted source changes before expensive final qualification. Preserve
explicit maintenance deferrals rather than merging dependencies just to empty
the queue. The broader evidence, product-journey, proof-economics, architecture,
and secondary-command lanes under
[#262](https://github.com/EffortlessMetrics/shipper-swarm/issues/262) and
[#277](https://github.com/EffortlessMetrics/shipper-swarm/issues/277) remain
post-0.5 unless #255 explicitly adopts a bounded prerequisite. The complete
product journey is still necessary to its own outcome; it is not silently a
new freeze gate. [#453](https://github.com/EffortlessMetrics/shipper-swarm/issues/453)
owns reconciliation of the remaining support-map and dated handoff projections.

## Five existential pillars (the safety claim)

Cargo stabilized multi-package workspace publishing. “Publish several crates at once” is not enough differentiation. Shipper is worth existing only if it owns five guarantees Cargo does not give the operator together:

| Pillar | Question it answers | Current state |
|---|---|---|
| **Prove** | Can I show this release is safe *before* the irreversible step? | Implemented base — deterministic plan, structured preflight, package dry-run, ownership evidence where applicable, registry pacing, policy reports, rehearsal, and versioned JSON evidence. Exact release-candidate proof is repeated for every release. |
| **Dispatch** | Is publication executed in a registry-aware, paced way? | Implemented for the evidence-backed crates.io profile — authoritative publish regimes, first-publish burst/refill behavior, `Retry-After` floors, readiness checks, and duration estimates. Additional registry profiles require their own evidence. |
| **Reconcile** | When the result is ambiguous, do I check registry truth before retrying? | Unknown-observation and retained-retry reconciliation corrections are merged through [PR #429](https://github.com/EffortlessMetrics/shipper-swarm/pull/429) and [PR #430](https://github.com/EffortlessMetrics/shipper-swarm/pull/430). Preserve their stop-on-unknown controls in exact-candidate proof; development implementation is not public-release qualification. |
| **Recover** | If the runner dies mid-train, can I converge from durable state without losing or duplicating work? | Whole-plan completion [PR #423](https://github.com/EffortlessMetrics/shipper-swarm/pull/423), retained-state admission [PR #436](https://github.com/EffortlessMetrics/shipper-swarm/pull/436), and repeated-continuation machinery [PR #432](https://github.com/EffortlessMetrics/shipper-swarm/pull/432) are merged. Native cleanup limits (#412), actual cross-run continuation (#418), and exact-candidate/release-authority proof remain; controlled interruption is not arbitrary host-loss certification. |
| **Remediate** | If a partial release goes bad, can I contain or fix-forward it mechanically? | Bounded implementation — receipt-driven yank/fix-forward planning, dry-run artifacts, guarded yank execution, and compromise evidence exist. Live registry mutation remains an explicit operator decision. |

The engine has the release-closure structure. Preserve the existing evidence
while repairing the observed contract gaps, finishing the 0.5 source-authority
release, and proving the actual auth and integration consumers. The dated
[product audit](https://github.com/EffortlessMetrics/shipper-swarm/issues/259)
records which claims were exercised and their limits.

## Nine competencies (the implementation ledger)

The five pillars cover the safety story. Narrate, Harden, Integrate, and Ergonomics make the tool legible, securable, embeddable, and approachable.

| # | Competency | Definition | Current state | Issue |
|---|---|---|---|---|
| 1 | **Prove** | Establish before the irreversible step that publication can succeed | Implemented base; repeated per release candidate | [#100](https://github.com/EffortlessMetrics/shipper/issues/100) |
| 2 | **Survive** | Recover from interruption without losing or duplicating work | Implemented base; current recovery gaps are tracked above | [#101](https://github.com/EffortlessMetrics/shipper/issues/101) |
| 3 | **Reconcile** | Close ambiguous outcomes against registry truth | Unknown-observation correction merged; exact-candidate proof remains | [#102](https://github.com/EffortlessMetrics/shipper/issues/102) |
| 4 | **Narrate** | Tell the operator what is happening live, not only afterward | Ordered live retry delivery merged via [PR #450](https://github.com/EffortlessMetrics/shipper-swarm/pull/450), with focused engine/CLI proof; public qualification remains separate | [#103](https://github.com/EffortlessMetrics/shipper/issues/103) |
| 5 | **Remediate** | Mechanically contain or fix-forward bad partial outcomes | Bounded implementation | [#104](https://github.com/EffortlessMetrics/shipper/issues/104) |
| 6 | **Harden** | Default to the proved auth posture and bound supply-chain evidence | Active — exact-source Trusted Publishing proof and provenance decision remain | [#105](https://github.com/EffortlessMetrics/shipper/issues/105) |
| 7 | **Profile** | Encode evidence-backed registry constraints and regimes | Implemented for crates.io | [#106](https://github.com/EffortlessMetrics/shipper/issues/106) |
| 8 | **Integrate** | Expose reliable machine and embedding boundaries | Bounded webhook lifecycle and external-style embedding proof remain in post-0.5 [#407](https://github.com/EffortlessMetrics/shipper-swarm/issues/407) / [#408](https://github.com/EffortlessMetrics/shipper-swarm/issues/408), unless explicitly admitted | [#107](https://github.com/EffortlessMetrics/shipper/issues/107) |
| 9 | **Ergonomics** | Keep first-run, diagnosis, and recovery friction low | Implemented base | [#108](https://github.com/EffortlessMetrics/shipper/issues/108) |

The historical master scorecard is [#109](https://github.com/EffortlessMetrics/shipper/issues/109). It is superseded by this roadmap and the current release program; it should not be used to infer present implementation status.

The important unresolved distinctions are narrow:

- Trusted Publishing is supported, but 0.4.0 selected `fallback_secret`; the default claim waits for exact-source rehearsal evidence.
- Webhooks exist and preserve sequential/parallel event parity, but delivery is still best-effort and not durably observable.
- The crates.io profile is implemented; another registry does not get a profile until its pacing and propagation semantics are evidenced.
- A completed competency does not waive release-candidate proof. Every irreversible release still re-establishes source identity, package surface, compatibility, auth, recovery, and public-result evidence.

## Design principles

### Reliability over speed

Default behaviors verify, log, and provide evidence. Faster paths are explicit opt-ins. The default publish policy (`safe`) includes the required verification.

### Determinism

Publish order is reproducible. Plan IDs are SHA-256 identities of the workspace plan and stable across equivalent environments. The same release inputs must produce the same plan identity.

### Events are truth; state is a projection

Per [docs/INVARIANTS.md](docs/INVARIANTS.md), `events.jsonl` is authoritative and append-only. `state.json` is a projection for resume convenience. `receipt.json` is a summary derived at end of run. Event/state/receipt drift is a product defect, not an acceptable reporting mismatch.

### Engine is library; CLI is an adapter

Release behavior lives in `crates/shipper-core` and its contract/adapter crates. `crates/shipper-cli` parses arguments and renders output. `crates/shipper` is the install facade and curated product-name re-export. Other frontends consume supported public boundaries rather than importing CLI internals.

### Evidence controls release authority

A green branch, merged PR, configuration value, or successful token mint is not publication authorization. The release issue must name one approved SHA/tree and one observed auth path. Public claims follow retained evidence, not intended configuration.

### Forbid unsafe; respect MSRV

`unsafe_code = "forbid"` is workspace-wide. Edition 2024 and MSRV 1.95 are policy boundaries, with repository checks preventing silent regression.

## Now / Next / Later

### Now — close the 0.5 line

1. **Finish bounded safety and consumer proof** — follow #412/#417/#418 and the source-context correction #454 above; preserve landed repairs and keep their remaining platform/transport claims explicit.
2. **Qualify one accepted candidate** — #255 owns queue disposition, complete exact-source proof, and freeze; #370 supplies the existing broad Linux route. Do not count queued, cancelled, skipped, unavailable, or earlier-source results as current passes.
3. **Complete the release-authority chain** — use [#475–#479](https://github.com/EffortlessMetrics/shipper/issues/475), including approved identity/date, observed auth posture under [#105](https://github.com/EffortlessMetrics/shipper/issues/105), exact-source rehearsal, explicit GO, publication, public verification, and backfill. Keep fallback explicit until the short-lived path is proved across the train.
4. **Demonstrate actual use** — retain the thirteen-crate consumer run and a supported follow-up inspection/rerun under [swarm #251](https://github.com/EffortlessMetrics/shipper-swarm/issues/251). RIPR adoption follows its existing read-only, identity, parity, and cutover prerequisites; release availability alone does not complete adoption.

Coverage and evidence maintenance continue under their existing owners: compile
every fuzz target, keep scheduled campaign outcomes honest, and maintain direct
tests at high-value policy seams. Additional provenance remains a threat-model
and consumer decision under #105, not a new tool checklist or implied gate.

### Next — extend only from evidence

1. Add a second registry profile only after its documented and observed pacing, propagation, ambiguity, and auth semantics can be encoded and tested.
2. Exercise bounded remediation against a deliberate non-production registry or equally strong harness before promoting live-registry automation.
3. Tighten document-contract checks after the current status/link drift is repaired; do not make a known-broken projection blocking first.
4. Deliver the bounded post-0.5 webhook lifecycle (#407) and supported embedding consumer (#408) under [#107](https://github.com/EffortlessMetrics/shipper/issues/107); improve notification replay or external adapters only after those contracts are stable.

### Later — concrete consumers, not generic integration theatre

- IDP adapters for Backstage, Port, Cortex, or another platform when an actual consumer supplies the required workflow and compatibility surface.
- HTTP or service APIs only when process embedding and versioned CLI/event contracts are insufficient for a real deployment.
- Additional provenance formats only when an independent verifier consumes them.
- Performance work after correctness evidence identifies a material operator cost.

## Explicit non-goals

The Shipper product does not own these concerns merely because the repository release workflow may orchestrate them around a release:

| Concern | Primary owner |
|---|---|
| Version bumping | release-plz, cargo-release, or an explicit repository process |
| Changelog generation | Changie/release tooling |
| Git tag creation | release-authority workflow or operator |
| GitHub Release publication | release-authority workflow / GitHub tooling |
| crates.io team management | `cargo owner` |
| Dependency updates | Cargo/Dependabot and reviewed maintenance PRs |

**Shipper focuses on safe workspace publication and evidence-backed convergence.** Repository automation may call adjacent tools, but those calls do not become engine responsibilities.

## Contributing

Work from the current open issue and PR queues, not the historical #100–#109 scorecard alone.

Prioritize changes by:

1. fit with the release-closure model;
2. whether they close a real proof, operator-confidence, or maintainability gap;
3. whether the claim can be challenged by tests or retained evidence;
4. maintenance burden relative to value.

A good PR is bounded, names the governing contract, carries direct tests or a reason they are inapplicable, updates the relevant policy/document projection, and does not claim publication authority.

## Version history

| Version | Date | Theme |
|---|---|---|
| v0.4.0 | 2026-05-20 | Stable release-closure line; idempotent workspace publish, JSON evidence envelopes, registry-truth reconciliation, resume proof, auth evidence, and bounded remediation surfaces |
| v0.3.0-rc.1 | 2026-04-16 | First crates.io publish; 12 crates live; deterministic plan, retry absorption, and evidence trail proven under real rate limits |
| v0.2.0 | 2026-02-14 | Evidence and verification: event log, receipts, readiness checks, publish policies |
| v0.1.0 | — | Initial release |
