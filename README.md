# shipper

`shipper` is a **publishing reliability layer** for Rust workspaces.

Cargo packages and uploads. The workflow around it — planning a multi-crate release, surviving rate limits, recovering when CI dies, knowing what actually shipped when the upload was ambiguous — is where things tend to break. Shipper makes a workspace publish **safe to start** and **safe to re-run**.

> **The single test:** you can start a release train, stop staring at the terminal, and still trust the outcome.

> **Why this exists:** see [MISSION.md](MISSION.md) for mission, vision, audience, and the convictions that shape every default.

## Status

**v0.3.0-rc.1 shipped 2026-04-16** — first real-world crates.io publish, 12 crates live, driven by Shipper itself. The publish absorbed 41 retries silently across a 69-minute run. See [ROADMAP.md](ROADMAP.md) for the post-rc.1 product thesis (nine competencies) and master tracking issue [#109](https://github.com/EffortlessMetrics/shipper/issues/109).

| Audience | Start here |
|---|---|
| New user | This README → [docs/release-runbook.md](docs/release-runbook.md) |
| Operator | [docs/release-runbook.md](docs/release-runbook.md), [docs/configuration.md](docs/configuration.md) |
| Contributor | [MISSION.md](MISSION.md) → [ROADMAP.md](ROADMAP.md) → [CONTRIBUTING.md](CONTRIBUTING.md) → an issue from #100–#109 |
| AI assistant | [CLAUDE.md](CLAUDE.md) or [GEMINI.md](GEMINI.md) |
| Auditing receipts/events | [docs/INVARIANTS.md](docs/INVARIANTS.md) |

## Install

From crates.io:

```bash
cargo install shipper-cli --locked
```

> The binary is named `shipper` but the published crate is `shipper-cli`. [#95](https://github.com/EffortlessMetrics/shipper/issues/95) tracks making `cargo install shipper` work.

From this repository:

```bash
cargo install --path crates/shipper-cli --locked
```

## Quick start

```bash
shipper plan        # preview the publish order
shipper preflight   # verify everything is ready
shipper publish     # execute the plan
shipper resume      # if interrupted, continue from the last state
```

`shipper --help` and `shipper <subcommand> --help` are the canonical command/flag reference.

For a walkthrough, see the [first-publish tutorial](docs/tutorials/first-publish.md). For the full docs tree (tutorials / how-to / reference / explanation), see [docs/README.md](docs/README.md).

## How it works

The core flow is **plan → preflight → publish → (resume if interrupted)**.

1. **Plan.** Reads the workspace via `cargo_metadata`, filters publishable crates, topologically sorts them by intra-workspace dependencies, and computes a SHA256-based `plan_id`. Same workspace state always produces the same `plan_id`.
2. **Preflight.** Validates git cleanliness, registry reachability, performs a workspace dry-run, checks version-not-taken, optionally verifies ownership. Produces a `Finishability` assessment (Proven / NotProven / Failed).
3. **Publish.** Executes the plan one crate at a time with retry/backoff. After each `cargo publish`, verifies registry visibility (sparse index and/or API) before advancing to a dependent crate. Persists state to disk after every step.
4. **Resume.** Reloads `.shipper/state.json`, validates the `plan_id` matches the current workspace, skips already-published packages, continues from the first pending crate.

State lives in `.shipper/`:

- **`events.jsonl`** — append-only event stream. **The authoritative record.**
- **`state.json`** — projection over events for fast resume.
- **`receipt.json`** — end-of-run summary with evidence.
- **`lock`** — concurrent-publish guard.

The truth/projection/summary contract is documented in [docs/INVARIANTS.md](docs/INVARIANTS.md). Use `--state-dir <path>` to redirect (e.g. into a CI artifacts directory).

## What shipper does

- Deterministic, dependency-ordered publish plan.
- Pre-flight checks (git, registry, dry-run, version, ownership).
- Per-crate publish with retry/backoff for retryable failures.
- Post-publish readiness verification before advancing to dependents.
- Resumable state after every step.
- Append-only audit event log + machine-readable receipt with evidence (stdout/stderr, exit codes, git context, environment fingerprint).
- Multi-registry orchestration in a single run (state segregated per registry).
- Parallel publishing for independent crates within the dependency graph.
- Configurable safety/speed tradeoff via publish policies (`safe`, `balanced`, `fast`).

## What shipper does not do

- Bump versions, generate changelogs, create git tags, or write release notes. Use [cargo-release](https://github.com/crate-ci/cargo-release) or [release-plz](https://github.com/MarcoIeni/release-plz) for those. Shipper picks up *after* the version is decided.

## Authentication

Publishing itself is performed by Cargo. Shipper resolves a registry token from the same places Cargo does:

1. `CARGO_REGISTRY_TOKEN` (crates.io)
2. `CARGO_REGISTRIES_<NAME>_TOKEN` (alternative registries; `<NAME>` uppercased, `-` replaced with `_`)
3. `$CARGO_HOME/credentials.toml`

The token is opaque, never logged, sanitized from receipts.

`shipper doctor` reports `auth_type`:

- `token` — Cargo token detected
- `trusted` — GitHub OIDC trusted-publishing env detected
- `unknown` — partial auth env detected
- `-` — none

> Trusted Publishing detection exists today; making it the default (with OIDC token exchange) is tracked at [#96](https://github.com/EffortlessMetrics/shipper/issues/96).

## Configuration

Project-specific configuration via `.shipper.toml` in the workspace root. CLI flags always take precedence.

```bash
shipper config init       # generate a default config file
shipper config validate   # validate a configuration file
```

See [docs/configuration.md](docs/configuration.md) for the full reference.

## CI integration

Generate a workflow snippet for your platform:

```bash
shipper ci github-actions
shipper ci gitlab
shipper ci circleci
shipper ci azure-devops
```

Or browse `templates/` for reference workflows.

## Examples

### Choosing a publish policy

```bash
shipper publish --policy safe       # default: verify every step
shipper publish --policy balanced   # verify when needed
shipper publish --policy fast       # skip verification (with caution)
```

### Choosing a readiness method

```bash
shipper publish --readiness-method api    # fast (default)
shipper publish --readiness-method index  # more accurate
shipper publish --readiness-method both   # most reliable
```

### Multi-registry publishing

```bash
shipper publish --registries crates-io,internal-mirror
shipper publish --all-registries
```

### Inspecting state and receipts

```bash
shipper inspect-events                  # human-readable event log
shipper inspect-receipt --format json   # JSON receipt for CI consumption
shipper status                          # compare local versions to the registry
```

### Resuming after interruption

```bash
shipper resume
shipper resume --force-resume           # if the workspace plan has changed
shipper publish --resume-from my-crate  # restart from a specific crate
```

## Workspace crates

- [`shipper-cli`](crates/shipper-cli/README.md) — installs the `shipper` binary.
- [`shipper`](crates/shipper/README.md) — reusable library: planning, preflight, publish, resume, receipts.

The full crate map is in [docs/structure.md](docs/structure.md).

## Documentation

The full docs tree is organized by reader purpose ([Diátaxis](https://diataxis.fr/)): tutorials, how-to guides, reference, explanation. Start at **[docs/README.md](docs/README.md)**.

Top of the repo:

- [MISSION.md](MISSION.md) — mission, vision, audience, beliefs
- [ROADMAP.md](ROADMAP.md) — five pillars, nine-competency scorecard, now/next/later
- [docs/README.md](docs/README.md) — documentation index

Quick links:

- **Learn:** [First publish tutorial](docs/tutorials/first-publish.md) • [Recover from interruption](docs/tutorials/recover-from-interruption.md)
- **Do:** [Run in GitHub Actions](docs/how-to/run-in-github-actions.md) • [Inspect state & receipts](docs/how-to/inspect-state-and-receipts.md)
- **Look up:** [CLI reference](docs/reference/cli.md) • [`.shipper.toml`](docs/configuration.md) • [Failure modes](docs/failure-modes.md)
- **Understand:** [Why Shipper](docs/explanation/why-shipper.md) • [Architecture](docs/architecture.md) • [Invariants](docs/INVARIANTS.md)

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license

at your option.
