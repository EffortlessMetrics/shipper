# shipper

Publishing a multi-crate Rust workspace is easy to start and hard to trust. Shipper gives you a deterministic plan, resumable execution, and an audit trail you can actually use when something goes sideways.

## What Shipper does

- Builds a deterministic, dependency-ordered publish plan.
- Runs preflight checks before the first irreversible step.
- Publishes one crate at a time, verifies visibility, then advances.
- Persists state after every step so interrupted runs resume cleanly.
- Reconciles ambiguous `cargo publish` outcomes against registry truth instead of blind-retrying.
- Records events, state, and receipts for post-run auditing and remediation.

## Try it

```bash
cargo install shipper --locked
shipper plan        # preview the publish order
shipper preflight   # check readiness
shipper publish     # execute the plan
shipper resume      # if interrupted, continue from the last state
```

## What Shipper does not do

Shipper does not decide version numbers, generate changelogs, tag releases, or create GitHub releases. Pair it with [cargo-release](https://github.com/crate-ci/cargo-release) or [release-plz](https://github.com/MarcoIeni/release-plz) for those; Shipper picks up after the version is decided.

## Where to go next

- **Learn** → [docs/tutorials](docs/tutorials) (first publish, recovery walkthrough)
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
