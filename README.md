# shipper

[![CI](https://github.com/EffortlessMetrics/shipper/actions/workflows/ci.yml/badge.svg)](https://github.com/EffortlessMetrics/shipper/actions/workflows/ci.yml)
[![Codecov](https://codecov.io/gh/EffortlessMetrics/shipper/branch/main/graph/badge.svg)](https://codecov.io/gh/EffortlessMetrics/shipper)
[![ripr](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/EffortlessMetrics/shipper/main/badges/ripr.json)](docs/ci/ripr.md)
[![ripr+](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/EffortlessMetrics/shipper/main/badges/ripr-plus.json)](docs/ci/ripr.md)
[![MSRV](https://img.shields.io/badge/MSRV-1.95-blue.svg)](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Publishing a multi-crate Rust workspace is easy to start and hard to trust. Shipper gives you a deterministic plan, resumable execution, and an audit trail you can actually use when something goes sideways.

## What Shipper does

- Builds a deterministic, dependency-ordered publish plan.
- Runs preflight checks before the first irreversible step.
- Publishes one crate at a time, verifies visibility, then advances.
- Persists state after every step so interrupted runs resume cleanly.
- Reconciles ambiguous `cargo publish` outcomes against registry truth instead of blind-retrying.
- Records events, state, and receipts for post-run auditing and remediation.

## Install

Shipper's supported install package is the product facade crate, `shipper`.
Because the public crates.io package is currently prerelease-only, Cargo needs
an explicit version when installing from the registry:

```bash
cargo install shipper --version 0.3.0-rc.2 --locked
shipper --version
```

Once a non-prerelease version is published, the stable install handle becomes:

```bash
cargo install shipper --locked
```

For local checkout validation before a release, use the same facade crate:

```bash
cargo install --path crates/shipper --locked
shipper --help
```

## Try it

```bash
shipper plan        # preview the publish order
shipper preflight   # check readiness
shipper publish     # execute the plan
shipper resume      # if interrupted, continue from the last state
```

## What Shipper does not do

Shipper does not decide version numbers, generate changelogs, tag releases, or create GitHub releases. Pair it with [cargo-release](https://github.com/crate-ci/cargo-release) or [release-plz](https://github.com/MarcoIeni/release-plz) for those; Shipper picks up after the version is decided.

## Where to go next

- **Learn** → [docs/tutorials](docs/tutorials) (five-minute confidence path, first publish, recovery walkthrough)
- **Do** → [docs/how-to](docs/how-to) (CI integration, stalled-run triage, remediation)
- **Look up** → [docs/reference](docs/reference) (CLI, state files, `.shipper.toml`)
- **Understand** → [docs/explanation](docs/explanation) (why Shipper, `not_proven`, invariants)
- **All docs** → [docs/README.md](docs/README.md)

## For embedders

Depend on [`shipper-core`](crates/shipper-core/README.md) — the engine library with no CLI dependencies. The [`shipper`](crates/shipper/README.md) crate is the install face; [`shipper-cli`](crates/shipper-cli/README.md) is the real CLI adapter. See [docs/structure.md](docs/structure.md) for the full crate map.

## Project

- [MISSION.md](MISSION.md) — mission, vision, audience
- [ROADMAP.md](ROADMAP.md) — nine-competency thesis, current status
- [CHANGELOG.md](CHANGELOG.md) — release history
- [CONTRIBUTING.md](CONTRIBUTING.md) — how to contribute

## License

Licensed under either of Apache-2.0 or MIT at your option.
