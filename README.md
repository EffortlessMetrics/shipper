<p align="center">
  <img src="assets/logo/shipper-container-plain.svg" alt="Shipper logo" width="128" />
</p>

<h1 align="center">shipper</h1>

<p align="center">
  <em>Idempotent, resumable publishing for Rust workspaces.</em>
</p>

<p align="center">
  <a href="https://github.com/EffortlessMetrics/shipper/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/EffortlessMetrics/shipper/actions/workflows/ci.yml/badge.svg?branch=main" /></a>
  <a href="https://codecov.io/gh/EffortlessMetrics/shipper"><img alt="Codecov" src="https://codecov.io/gh/EffortlessMetrics/shipper/branch/main/graph/badge.svg" /></a>
  <a href="docs/ci/ripr.md"><img alt="ripr+" src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/EffortlessMetrics/shipper/main/badges/ripr-plus.json" /></a>
</p>

<p align="center">
  <a href="https://github.com/EffortlessMetrics/shipper/releases"><img alt="GitHub release" src="https://img.shields.io/github/v/release/EffortlessMetrics/shipper?sort=semver&label=release" /></a>
  <a href="https://crates.io/crates/shipper"><img alt="crates.io downloads" src="https://img.shields.io/crates/d/shipper.svg?label=crates.io%20downloads" /></a>
  <a href="https://docs.rs/shipper"><img alt="docs.rs" src="https://docs.rs/shipper/badge.svg" /></a>
</p>

<p align="center">
  <a href="https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.95-blue.svg" /></a>
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" /></a>
</p>

Shipper publishes the missing `name@version` pairs in a Rust workspace, in
dependency order. It verifies registry visibility before advancing, records
durable evidence, and can resume an interrupted run without blindly uploading
the same version again.

## Why Shipper

`cargo publish --workspace` is non-atomic. A run can stop after publishing only
part of a workspace, and a client timeout does not prove whether an upload
succeeded. Shipper owns that narrow operational gap:

- versions, changelogs, and tags are already chosen;
- existing versions are skipped only after registry confirmation;
- ambiguous Cargo results are reconciled against registry truth;
- a `StillUnknown` result stops for operator action instead of blind retry;
- events, state, and receipts explain what happened and support safe recovery.

Shipper does not choose versions, generate changelogs, create tags, or publish
GitHub Releases. Use it with the release-planning workflow you already trust.

## Install

Install the latest version currently exposed by the public registry:

```bash
cargo install shipper --locked
```

The retained public-install evidence available when this source snapshot was
prepared covers 0.4.0. Pin that verified baseline for a reproducible install:

```bash
cargo install shipper --version 0.4.0 --locked
```

The unversioned command resolves whatever version the registry exposes when it
runs; source text alone does not prove that a candidate was published. This
snapshot was prepared for 0.5.0 before its public result existed. See the live
registry/release and [support tiers](docs/status/SUPPORT_TIERS.md) for the exact
stable, internal, advisory, and planned boundaries.

To exercise the current candidate from a checkout:

```bash
cargo run -p shipper -- --help
```

## First useful path

Start with reversible evidence before the irreversible publish step:

```bash
shipper doctor      # diagnose workspace, auth, and registry posture
shipper plan        # show the dependency-ordered publication graph
shipper preflight   # assess readiness as PROVEN / NOT PROVEN / FAILED
shipper publish     # publish missing versions and retain evidence
```

After a run:

```bash
shipper status          # registry-aware status view
shipper inspect-events  # chronological event detail
shipper inspect-receipt # summarized retained evidence
shipper resume          # continue only after the retained evidence agrees
```

The 0.5.0 candidate also provides `shipper status --durable`, a local,
registry-bypassing view derived from retained evidence. It fails closed when
identity, evidence, or liveness is inconclusive; an old or missing lock is not
by itself permission to resume. The current command surface is always
`shipper --help` and `shipper <command> --help`.

## Evidence and recovery

A run writes its evidence under `.shipper/` (or the configured state
directory):

| Artifact | Authority |
|---|---|
| `events.jsonl` | Append-only authoritative truth. |
| `state.json` | Resumable projection of the events. |
| `receipt.json` | Derived end-of-run summary. |
| `reconciliation.json` | Registry-truth evidence for ambiguous outcomes. |

If these sources disagree, stop and investigate; events win, and drift is a
bug. Start with the [inspection guide](docs/how-to/inspect-state-and-receipts.md)
or the [interruption recovery tutorial](docs/tutorials/recover-from-interruption.md).

## Crate roles

Most users install `shipper`. The workspace keeps the product facade, command
adapter, and reusable engine separate:

| Crate | Role |
|---|---|
| [`shipper`](crates/shipper/README.md) | Install facade, binary, and curated product-name library re-exports. |
| [`shipper-cli`](crates/shipper-cli/README.md) | Clap command adapter, rendering, exit behavior, and `pub fn run()`. |
| [`shipper-core`](crates/shipper-core/README.md) | Reusable publishing engine without CLI dependencies. |

Direct `shipper-cli` embedding is specialized. Engine consumers should normally
use `shipper-core` or the curated `shipper` re-exports.

## Documentation

| Need | Start here |
|---|---|
| Learn the documentation journey | [Documentation index](docs/README.md) |
| Publish missing workspace crates | [Publishing guide](docs/how-to/publish-missing-workspace-crates.md) |
| Run in GitHub Actions | [GitHub Actions guide](docs/how-to/run-in-github-actions.md) |
| Recover after interruption | [Recovery tutorial](docs/tutorials/recover-from-interruption.md) |
| Look up commands and flags | [CLI reference](docs/reference/cli.md) |
| Configure Shipper | [Configuration reference](docs/configuration.md) |
| Understand failure modes | [Failure modes](docs/failure-modes.md) |
| Review 0.5 migration impact | [0.5.0 migration guide](docs/release/0.5.0-migration.md) |
| Review the prepared release story | [0.5.0 release notes](RELEASE_NOTES_v0.5.0.md) |
| Check supported claims | [Support tiers](docs/status/SUPPORT_TIERS.md) |

## Repository split

Active development targets
[`EffortlessMetrics/shipper-swarm`](https://github.com/EffortlessMetrics/shipper-swarm).
[`EffortlessMetrics/shipper`](https://github.com/EffortlessMetrics/shipper)
remains the release authority for crates.io publishing, tags, release evidence,
and signing credentials until that authority is deliberately moved.

No tag, publication, GitHub Release, signing, deployment, or credential action
is implied by source preparation in this repository.

## Project

- [Mission](MISSION.md)
- [Roadmap](ROADMAP.md)
- [Changelog](CHANGELOG.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

Licensed under either of Apache-2.0 or MIT at your option.
