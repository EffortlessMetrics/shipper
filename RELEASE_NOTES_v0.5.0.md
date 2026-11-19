# Shipper 0.5.0 release notes

Status: **reviewed release body; this source file grants no publication authority**

Shipper 0.5.0 makes Rust workspace publication easier to recover and easier to
operate. Durable events now capture the upload-to-visibility window and
attempt history, publish and resume return consistent typed outcomes, and
registry and credential checks stop unsafe work earlier.

## Why this release matters

- Interrupted publishes can resume from durable uploaded and reconciliation
  checkpoints instead of relying on process memory or repeating an upload.
- Sequential and parallel modes share one package-execution authority and the
  same readiness, timeout, retry, resume-skip, and evidence behavior.
- Human and JSON output use the same next-action and rerun/resume semantics.
  `shipper status --durable` provides a network-free, fail-closed view of local
  run evidence.
- Registry destination, loopback/private-network, selector, credential, and
  strict-ownership validation occurs before irreversible package work.
- Real 0.4 durable artifacts, including encrypted v1 state, remain readable
  and are covered by retained replay/resume evidence.

## Before upgrading

Read the [0.5.0 migration guide](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/release/0.5.0-migration.md) if you:

- construct `RuntimeOptions` or `Receipt` with a Rust struct literal;
- construct or match `EventType::PackageAttempted`, or exhaustively match
  public event or state enums;
- parse `events.jsonl`, `state.json`, `receipt.json`, or command JSON;
- depend on exit status `2` or generate resume commands;
- use custom, private, loopback, or multiple registries;
- read or write encrypted state.

### Installation posture

This reviewed source file does not prove that 0.5.0 is publicly available. If
you are reading it on the published `v0.5.0` GitHub Release produced after the
release workflow completed, install the CLI with:

```console
cargo install shipper --version 0.5.0 --locked
```

Before that workflow-created Release exists, treat the command as prospective
upgrade guidance and evaluate only a reviewed source candidate in a disposable
environment. Preserve active `.shipper` evidence before changing the binary
used to inspect or resume a run.

The short compatibility summary is:

- old durable artifacts remain readable within the tested contract;
- new Rust fields and variants require source updates in some consumers; do
  not resume journals containing new 0.5 records with a 0.4 binary;
- new encrypted artifacts use KDF v2 while v1 reads remain supported;
- `parallel.per_package_timeout` remains the shared timeout key for one
  migration cycle;
- a Shipper partial-result exit `2` is distinct from Clap's usage-error exit
  `2` and carries a command-owned outcome envelope.

## Operating boundaries

- Linux can report an exact live local publisher only when boot identity, PID
  namespace, PID, and process-start identity all match. Other platforms and
  incomplete, legacy, cross-host, or contradictory evidence fail closed to an
  unknown posture.
- Existing lock acquisition still uses its documented age-based timeout; the
  durable observation work does not make lock stealing liveness-aware.
- Raw inspect-event JSONL and direct receipt serialization remain governed
  evidence drilldowns, not a new aggregate inspect schema.
- Fixture, replay, and hosted CI evidence do not prove a real-registry publish,
  public availability, every platform, or a successful cold walkthrough.

## Security and maintenance

This release includes the `h2` 0.4.16 remediation for RUSTSEC-2026-0258,
updates `quinn-proto` for its upstream security fix, updates `serde_with` as
defense in depth, strengthens KDF parameters for new encrypted writes, and
preserves secret redaction across operator diagnostics.
The release workflow also fails closed on approved source, reviewed notes,
binary ordering, workflow authority, and resume identity.

## Evidence and next steps

- [Full changelog](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/CHANGELOG.md)
- [Migration guide](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/release/0.5.0-migration.md)
- [Evidence-backed change ledger](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/release/0.5.0-change-ledger.md)
- [Readiness evidence](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/release/0.5.0-readiness.md)
- [Release preparation status](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/release/0.5.0-preparation.md)
- [Stalled-run inspection and recovery](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/how-to/inspect-a-stalled-run.md)

These release-authority, tag-qualified links become live only after the
separate `v0.5.0` tag action succeeds. They are intentionally not evidence
that the tag or public release exists today.

Before [release-authority rehearsal and GO/NO-GO (#476)](https://github.com/EffortlessMetrics/shipper/issues/476),
the [release-authority handoff (#478)](https://github.com/EffortlessMetrics/shipper/issues/478)
records the intended tag date and exact approved source commit and tree. That
handoff does not authorize publication. The actual tag is created only after a
separate GO; crates.io results, downloadable release binaries, the GitHub
Release, signatures, provenance, and public-install proof are recorded only
as the release workflow and public verification complete.
