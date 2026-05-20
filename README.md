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

Shipper publishes missing `name@version` pairs in dependency order, skips versions already on the registry, verifies visibility, records evidence, and resumes cleanly after interruption.

## The problem

`cargo publish --workspace` works when every package version is new. It gets awkward when CI reruns after a partial publish, when some versions already exist, or when Cargo exits ambiguously after an upload.

Most teams either script registry checks themselves or adopt heavier release automation that also decides versions, changelogs, tags, and releases.

Shipper is the narrow tool for the middle: versions are already chosen, and you need CI-safe workspace publishing.

## What works

- **Idempotent workspace publish**: skips already-published `name@version` pairs and publishes missing versions in dependency order.
- **Preflight proof**: checks local readiness, registry reachability, auth signals, dry-run status, ownership where possible, and registry pacing.
- **Resumable execution**: persists state after each step so interrupted runs can continue without blind duplicate publish attempts.
- **Ambiguous-result reconciliation**: checks registry truth before retrying after unclear Cargo outcomes.
- **Evidence packet**: records state, events, receipts, and reconciliation artifacts for CI, operators, and future remediation.
- **Bounded remediation**: yank planning, fix-forward planning, dry-run artifacts, and guarded fake-Cargo execution are proof-backed surfaces; live crates.io yank and fix-forward execution remain deliberately bounded.

## Install

The stable 0.4.0 install path is:

```bash
cargo install shipper --locked
```

For a reproducible 0.4.0 install, pin the version:

```bash
cargo install shipper --version 0.4.0 --locked
```

The public crates.io install path was smoke-tested after `v0.4.0` published.
`docs/status/SUPPORT_TIERS.md` remains the source of truth for install-support
status.

For local checkout validation before a release, use the same facade crate:

```bash
cargo install --path crates/shipper --locked
shipper --help
```

## First useful run

| Job | Start here |
|---|---|
| Publish missing workspace crate versions | [docs/how-to/publish-missing-workspace-crates.md](docs/how-to/publish-missing-workspace-crates.md) |
| Run in GitHub Actions | [docs/how-to/run-in-github-actions.md](docs/how-to/run-in-github-actions.md) |
| Recover after interruption | [docs/tutorials/recover-from-interruption.md](docs/tutorials/recover-from-interruption.md) |
| Inspect what happened | [docs/how-to/inspect-state-and-receipts.md](docs/how-to/inspect-state-and-receipts.md) |
| Diagnose auth / environment | `shipper doctor` |
| Embed Shipper in a Rust tool | [crates/shipper-core/README.md](crates/shipper-core/README.md) |

## Status at a glance

The README is a front door, not the source of truth. Current release posture, supported claims, CI evidence, and remediation readiness live in status docs.

| Area | Source |
|---|---|
| Release posture | [docs/release/](docs/release/) |
| Supported claims | [docs/status/SUPPORT_TIERS.md](docs/status/SUPPORT_TIERS.md) |
| Idempotent workspace publish | [docs/how-to/publish-missing-workspace-crates.md](docs/how-to/publish-missing-workspace-crates.md) |
| JSON evidence contracts | [docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md](docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md) |
| Operator visibility / survive proof | [docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md](docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md) |
| Auth evidence / Trusted Publishing | [docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md](docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md) |
| Receipt-driven remediation | [docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md](docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md) |

## Evidence packet

A publish run leaves an evidence packet under `.shipper/`:

| Artifact | Purpose |
|---|---|
| `state.json` | Resumable progress projection. |
| `events.jsonl` | Append-only release event log / black-box recorder. |
| `receipt.json` | Final release receipt. |
| `reconciliation.json` | Registry-truth evidence for ambiguous outcomes. |
| `auth-evidence.json` | Workflow auth/fallback evidence when release workflow records it. |
| `remediation-plan.json` | Receipt-driven containment/fix-forward plan from `shipper remediate --dry-run`. |

## Crate surface

Most users install `shipper`. Embedders depend on `shipper-core`.

| Need | Crate |
|---|---|
| Install the product CLI | [`shipper`](crates/shipper/README.md) |
| Use the CLI adapter directly | [`shipper-cli`](crates/shipper-cli/README.md) |
| Embed the release engine | [`shipper-core`](crates/shipper-core/README.md) |
| Shared serializable types | `shipper-types` |
| Registry/index helpers | `shipper-registry`, `shipper-sparse-index` |
| Config/runtime helpers | `shipper-config`, `shipper-duration`, `shipper-retry` |
| Evidence / safety helpers | `shipper-output-sanitizer`, `shipper-cargo-failure`, `shipper-encrypt`, `shipper-webhook` |

## Documentation

| Task | Link |
|---|---|
| Publish missing workspace crates | [docs/how-to/publish-missing-workspace-crates.md](docs/how-to/publish-missing-workspace-crates.md) |
| Run in GitHub Actions | [docs/how-to/run-in-github-actions.md](docs/how-to/run-in-github-actions.md) |
| Recover from interruption | [docs/tutorials/recover-from-interruption.md](docs/tutorials/recover-from-interruption.md) |
| Inspect state and receipts | [docs/how-to/inspect-state-and-receipts.md](docs/how-to/inspect-state-and-receipts.md) |
| CLI reference | [docs/reference/cli.md](docs/reference/cli.md) |
| Configuration | [docs/configuration.md](docs/configuration.md) |
| Failure modes | [docs/failure-modes.md](docs/failure-modes.md) |
| Support tiers | [docs/status/SUPPORT_TIERS.md](docs/status/SUPPORT_TIERS.md) |
| Roadmap | [ROADMAP.md](ROADMAP.md) |
| Contributing | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Agent workflow | [AGENTS.md](AGENTS.md) |

## How this project is built

Shipper uses a proof-first development conveyor: specs, plans, active goals, support-tier claims, focused PRs, CI evidence, and release artifacts.

The short version: user-facing claims must point to proof. If a claim is advisory, the docs say so. If a release step is irreversible, Shipper records evidence before and after it.

| Topic | Link |
|---|---|
| Source-of-truth stack | [docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md](docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md) |
| Support tiers | [docs/status/SUPPORT_TIERS.md](docs/status/SUPPORT_TIERS.md) |
| Active goal | [.shipper-meta/goals/active.toml](.shipper-meta/goals/active.toml) |
| Agent workflow | [AGENTS.md](AGENTS.md) |
| Contributor workflow | [CONTRIBUTING.md](CONTRIBUTING.md) |

## Verification posture

Shipper is built with strict Rust 1.95 policy rails, doc-contract checks, file/process/network policy ledgers, no-panic tracking, advisory ripr static mutation-exposure analysis, and release-readiness proof artifacts.

See [docs/status/SUPPORT_TIERS.md](docs/status/SUPPORT_TIERS.md) and [docs/ci/](docs/ci/).

## Security and release evidence

Shipper redacts token values from release evidence. Release workflow auth evidence records observed auth mode and fallback state without storing secrets. Trusted Publishing remains a promoted default only after release evidence proves that path.

See [docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md](docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md).

## Project

- [MISSION.md](MISSION.md) — mission, vision, audience
- [ROADMAP.md](ROADMAP.md) — nine-competency thesis, current status
- [CHANGELOG.md](CHANGELOG.md) — release history
- [CONTRIBUTING.md](CONTRIBUTING.md) — how to contribute

## License

Licensed under either of Apache-2.0 or MIT at your option.
