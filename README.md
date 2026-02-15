# shipper

`shipper` is a **publishing reliability layer** for Rust workspaces.

Cargo already knows how to package and upload crates. What tends to break in real life is the workflow around it:

- publishing is **irreversible** (versions can't be overwritten)
- multi-crate publishing can be **non-atomic** (partial publishes)
- CI runs get cancelled or time out
- registries apply **backpressure** (HTTP 429)
- some failures are **ambiguous** (upload may have succeeded even when the client errors)

Shipper is intentionally narrow: it focuses on making publishing **safe to start** and **safe to re-run**.

## What shipper does

- Builds a deterministic **publish plan** for workspace crates (dependency-first ordering).
- Runs **preflight checks** (git cleanliness, publishability, registry reachability, version existence).
- Optionally verifies **crate ownership/permissions** up front (when a token is available).
- Publishes **one crate at a time** using `cargo publish -p <crate>`.
- Applies retry/backoff for retryable failures.
- Verifies publish completion via the registry API before declaring success.
- Persists progress to disk so you can **resume** after interruption.
- Captures **evidence** for each operation (stdout, stderr, exit codes).
- Maintains an **event log** for complete audit trails.
- Performs **readiness checks** to ensure registry visibility before publishing dependent crates.
- Supports **parallel publishing** for independent packages within the dependency graph.
- Supports configurable **publish policies** for different safety levels.

## What shipper does not do (yet)

- It does not bump versions, generate changelogs, create git tags, or create GitHub releases.
  Use `cargo-release`, `release-plz`, or your own workflow to decide *what* version to publish.
  Shipper focuses on *getting those versions published reliably*.

## Build / install

From this repository:

```bash
cargo build --release
cargo install --path crates/shipper-cli --locked
```

## Quick start

```bash
shipper plan        # preview the publish order
shipper preflight   # verify everything is ready
shipper publish     # execute the plan
```

If the publish is interrupted (CI cancellation, network issues):

```bash
shipper resume
```

## Authentication

Publishing itself is performed by Cargo.

Shipper's registry API checks (version existence, optional owners preflight) resolve a token using the same places people already use for Cargo:

- `CARGO_REGISTRY_TOKEN` (crates.io)
- `CARGO_REGISTRIES_<NAME>_TOKEN` (other registries; `<NAME>` uppercased, `-` replaced with `_`)
- `$CARGO_HOME/credentials.toml` (created by `cargo login`)

The token is treated as an **opaque string** and sent as the value of the `Authorization` header, matching Cargo's registry web API model.

## State + receipts

By default Shipper writes:

- `.shipper/state.json` — resumable execution state
- `.shipper/receipt.json` — machine-readable receipt for CI/auditing
- `.shipper/events.jsonl` — detailed event log for debugging

Use `--state-dir <path>` to redirect these elsewhere (for example, a CI artifacts directory).

## Commands

### Core commands

- `shipper plan` — print the publish order and what will be skipped
- `shipper preflight` — run checks without publishing
- `shipper publish` — execute the plan (writes state + receipt + events)
- `shipper resume` — continue from the last state
- `shipper status` — compare local versions to the registry
- `shipper doctor` — environment and auth diagnostics

### Inspection commands

- `shipper inspect-events` — view detailed event log with timestamps and evidence
- `shipper inspect-receipt` — view detailed receipt with captured evidence
- `shipper clean` — clean state files (state.json, receipt.json, events.jsonl)

### CI commands

- `shipper ci github-actions` — print GitHub Actions workflow snippet
- `shipper ci gitlab` — print GitLab CI workflow snippet

### Configuration commands

- `shipper config init` — generate a default `.shipper.toml` configuration file
- `shipper config validate` — validate a configuration file

## Options

### Global options

- `--config <path>` — Path to a custom `.shipper.toml` configuration file
- `--manifest-path <path>` — Path to the workspace Cargo.toml (default: Cargo.toml)
- `--registry <name>` — Cargo registry name (default: crates-io)
- `--api-base <url>` — Registry API base URL (default: https://crates.io)
- `--package <name>` — Restrict to specific packages (repeatable)
- `--format <format>` — Output format: text (default) or json

### State options

- `--state-dir <path>` — Directory for shipper state and receipts (default: .shipper)
- `--force` — Force override of existing locks (use with caution)
- `--lock-timeout <duration>` — Lock timeout duration (default: 1h)

### Verification options

- `--policy <policy>` — Publish policy: safe (verify+strict), balanced (verify when needed), fast (no verify) (default: safe)
- `--verify-mode <mode>` — Verify mode: workspace (default), package (per-crate), none (no verify)
- `--no-verify` — Pass --no-verify to cargo publish

### Readiness options

- `--readiness-method <method>` — Readiness check method: api (default, fast), index (slower, more accurate), both (slowest, most reliable)
- `--readiness-timeout <duration>` — How long to wait for registry visibility during readiness checks (default: 5m)
- `--readiness-poll <duration>` — Poll interval for readiness checks (default: 2s)
- `--no-readiness` — Disable readiness checks (for advanced users)

### Preflight options

- `--allow-dirty` — Allow publishing from a dirty git working tree
- `--skip-ownership-check` — Skip owners/permissions preflight
- `--strict-ownership` — Fail preflight if ownership checks fail or if no token is available

### Retry options

- `--max-attempts <number>` — Max attempts per crate publish step (default: 6)
- `--base-delay <duration>` — Base backoff delay (default: 2s)
- `--max-delay <duration>` — Max backoff delay (default: 2m)
- `--verify-timeout <duration>` — How long to wait for registry visibility after a successful publish (default: 2m)
- `--verify-poll <duration>` — Poll interval for checking registry visibility (default: 5s)

### Evidence options

- `--output-lines <number>` — Number of output lines to capture for evidence (default: 50)

### Parallel options

- `--parallel` — Enable parallel publishing (packages at the same dependency level published concurrently)
- `--max-concurrent <number>` — Maximum concurrent publish operations (default: 4, implies --parallel)
- `--per-package-timeout <duration>` — Timeout per package in parallel mode (default: 30m)

### Resume options

- `--force-resume` — Force resume even if the computed plan differs from the state file

## Configuration file

Shipper supports project-specific configuration via a `.shipper.toml` file in your workspace root. CLI flags always take precedence over configuration file values.

```bash
shipper config init       # generate a default config file
shipper config validate   # validate an existing config file
```

See [docs/configuration.md](docs/configuration.md) for the full reference.

## Examples

### Basic publish workflow

```bash
shipper plan
shipper preflight
shipper publish
```

### Publish policies

```bash
# Safe mode (default): verify every publish with strict checks
shipper publish --policy safe

# Balanced mode: verify only when needed
shipper publish --policy balanced

# Fast mode: skip verification (use with caution)
shipper publish --policy fast
```

### Readiness checks

```bash
# API-based readiness (fast, default)
shipper publish --readiness-method api

# Index-based readiness (slower but more accurate)
shipper publish --readiness-method index

# Both methods (slowest but most reliable)
shipper publish --readiness-method both

# Custom timeout and poll interval
shipper publish --readiness-timeout 10m --readiness-poll 5s

# Disable readiness checks (advanced users only)
shipper publish --no-readiness
```

### Verify modes

```bash
# Verify at workspace level (default)
shipper publish --verify-mode workspace

# Verify each package individually
shipper publish --verify-mode package

# Skip verification
shipper publish --verify-mode none
```

### Inspecting events and receipts

```bash
# View the event log
shipper inspect-events

# View the detailed receipt with evidence
shipper inspect-receipt

# Get JSON output for CI integration
shipper inspect-receipt --format json
```

### Resuming after interruption

```bash
# Resume normally
shipper resume

# Force resume if plan has changed
shipper resume --force-resume
```

### Parallel publishing

```bash
# Publish independent packages concurrently
shipper publish --parallel

# Limit to 2 concurrent operations
shipper publish --parallel --max-concurrent 2

# With per-package timeout
shipper publish --parallel --per-package-timeout 10m
```

### Cleaning state files

```bash
# Clean all state files
shipper clean

# Keep the receipt but clean state and events
shipper clean --keep-receipt
```

## CI templates

See `templates/` for example workflows, or generate snippets:

```bash
shipper ci github-actions
shipper ci gitlab
```

## Documentation

- [Configuration](docs/configuration.md) - Configuration file reference
- [Preflight Verification](docs/preflight.md) - Pre-flight verification guide
- [Readiness Checking](docs/readiness.md) - Readiness verification guide
- [Failure Modes](docs/failure-modes.md) - Common failure scenarios and solutions

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license

at your option.
