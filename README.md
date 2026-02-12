# shipper

`shipper` is a **publishing reliability layer** for Rust workspaces.

Cargo already knows how to package and upload crates. What tends to break in real life is the workflow around it:

- publishing is **irreversible** (versions can't be overwritten)
- multi-crate publishing can be **non-atomic** (partial publishes)
- CI runs get cancelled or time out
- registries apply **backpressure** (HTTP 429)
- some failures are **ambiguous** (upload may have succeeded even when the client errors)

Shipper is intentionally narrow: it focuses on making publishing **safe to start** and **safe to re-run**.

## The Four Pillars of v0.2

Shipper v0.2 introduces four key pillars for reliable publishing:

1. **Evidence Capture** - Every publish operation captures detailed evidence (stdout/stderr, exit codes, timestamps) for debugging and auditing. Use `inspect-events` and `inspect-receipt` to review captured evidence.

2. **Event Logging** - A comprehensive event log (`events.jsonl`) records every step of the publishing process, making it easy to trace exactly what happened and when.

3. **Readiness Checks** - Configurable readiness verification ensures published crates are actually available on the registry before proceeding. Supports API, index, and combined verification methods with configurable timeouts.

4. **Publish Policies** - Three built-in policies control verification behavior: `safe` (verify+strict), `balanced` (verify when needed), and `fast` (no verify). Choose the right balance of safety and speed for your workflow.

## New Features in v0.2

### Preflight Verification

Preflight checks run before any publishing begins to verify your workspace is ready:

- **Finishability Assessment** - Determines if your workspace is ready to publish (Proven/NotProven/Failed)
- **Ownership Verification** - Checks if you have permission to publish each crate
- **New Crate Detection** - Identifies crates that don't exist on the registry yet
- **Workspace Dry-Run** - Verifies all packages can be published without uploading

```bash
# Run preflight checks
shipper preflight

# Run with strict ownership checks
shipper preflight --strict-ownership
```

See [docs/preflight.md](docs/preflight.md) for detailed documentation.

### Index-Based Readiness

New readiness verification methods for more reliable publishing:

- **API Method** (fast) - Queries the registry HTTP API
- **Index Method** (accurate) - Checks the sparse index directly
- **Both Method** (reliable) - Verifies using both methods

```bash
# Use index-based readiness
shipper publish --readiness-method index

# Use both methods for maximum reliability
shipper publish --readiness-method both
```

See [docs/readiness.md](docs/readiness.md) for detailed documentation.

### Enhanced Receipts

Receipts now include comprehensive evidence for debugging and auditing:

- **Attempt Evidence** - Stdout/stderr, exit codes, and duration for each attempt
- **Readiness Evidence** - Timestamps and results of each readiness check
- **Schema Versioning** - Receipts include version information for compatibility
- **Git Context** - Optional git commit, branch, and tag information

```bash
# View the detailed receipt with evidence
shipper inspect-receipt

# Get JSON output for CI integration
shipper inspect-receipt --format json
```

### Schema Versioning

State and receipt files now include version information for forward compatibility:

- **State Version** - Identifies the state file format version
- **Plan Version** - Identifies the plan format version
- **Receipt Version** - Identifies the receipt format version

This allows Shipper to handle format changes gracefully and provide clear migration paths.

### CI Integration Improvements

New CI commands for easy workflow generation:

```bash
# Get GitHub Actions workflow
shipper ci github-actions

# Get GitLab CI workflow
shipper ci gitlab
```

See [templates/](templates/) for example workflows.

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
- Performs **readiness checks** to ensure registry visibility.
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

## Quick start (local)

```bash
shipper plan
shipper preflight
shipper publish
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

### Inspection commands (v0.2)

- `shipper inspect-events` — view detailed event log with timestamps and evidence
- `shipper inspect-receipt` — view detailed receipt with captured evidence
- `shipper clean` — clean state files (state.json, receipt.json, events.jsonl)

### CI commands (v0.2)

- `shipper ci github-actions` — print GitHub Actions workflow snippet
- `shipper ci gitlab` — print GitLab CI workflow snippet

## Options

### Workspace options

- `--manifest-path <path>` — Path to the workspace Cargo.toml (default: Cargo.toml)
- `--registry <name>` — Cargo registry name (default: crates-io)
- `--api-base <url>` — Registry API base URL (default: https://crates.io)
- `--package <name>` — Restrict to specific packages (repeatable)

### State options

- `--state-dir <path>` — Directory for shipper state and receipts (default: .shipper)
- `--force` — Force override of existing locks (use with caution)
- `--lock-timeout <duration>` — Lock timeout duration (default: 1h)

### Verification options (v0.2)

- `--policy <policy>` — Publish policy: safe (verify+strict), balanced (verify when needed), fast (no verify) (default: safe)
- `--verify-mode <mode>` — Verify mode: workspace (default), package (per-crate), none (no verify)
- `--no-verify` — Pass --no-verify to cargo publish

### Readiness options (v0.2)

- `--readiness-method <method>` — Readiness check method: api (default, fast), index (slower, more accurate), both (slowest, most reliable)
- `--readiness-timeout <duration>` — How long to wait for registry visibility during readiness checks (default: 5m)
- `--readiness-poll <duration>` — Poll interval for readiness checks (default: 2s)
- `--no-readiness` — Disable readiness checks (for advanced users)

### Evidence options (v0.2)

- `--output-lines <number>` — Number of output lines to capture for evidence (default: 50)
- `--format <format>` — Output format: text (default) or json

### Preflight options

- `--allow-dirty` — Allow publishing from a dirty git working tree
- `--skip-ownership-check` — Skip owners/permissions preflight
- `--strict-ownership` — Fail preflight if ownership checks fail or if no token is available
- `--allow-new-crates` — Allow publishing new crates (first-time publishes)
- `--require-ownership-for-new-crates` — Require ownership verification for new crates

### Retry options

- `--max-attempts <number>` — Max attempts per crate publish step (default: 6)
- `--base-delay <duration>` — Base backoff delay (default: 2s)
- `--max-delay <duration>` — Max backoff delay (default: 2m)
- `--verify-timeout <duration>` — How long to wait for registry visibility after a successful publish (default: 2m)
- `--verify-poll <duration>` — Poll interval for checking registry visibility (default: 5s)

### Resume options

- `--force-resume` — Force resume even if the computed plan differs from the state file

## Examples

### Basic publish workflow

```bash
# Plan the publish
shipper plan

# Run preflight checks
shipper preflight

# Publish all crates
shipper publish
```

### Using publish policies (v0.2)

```bash
# Safe mode (default): verify every publish with strict checks
shipper publish --policy safe

# Balanced mode: verify only when needed
shipper publish --policy balanced

# Fast mode: skip verification (use with caution)
shipper publish --policy fast
```

### Configuring readiness checks (v0.2)

```bash
# Use API-based readiness (fast, default)
shipper publish --readiness-method api

# Use index-based readiness (slower but more accurate)
shipper publish --readiness-method index

# Use both methods (slowest but most reliable)
shipper publish --readiness-method both

# Custom timeout and poll interval
shipper publish --readiness-timeout 10m --readiness-poll 5s

# Disable readiness checks (advanced users only)
shipper publish --no-readiness
```

### Inspecting events and receipts (v0.2)

```bash
# View the event log
shipper inspect-events

# View the detailed receipt with evidence
shipper inspect-receipt

# Get JSON output for CI integration
shipper inspect-receipt --format json
```

### Cleaning state files (v0.2)

```bash
# Clean all state files
shipper clean

# Keep the receipt but clean state and events
shipper clean --keep-receipt
```

### Preflight verification (v0.2)

```bash
# Run preflight checks
shipper preflight

# Run with strict ownership checks
shipper preflight --strict-ownership

# Skip ownership checks
shipper preflight --skip-ownership-check

# Allow new crate publishing
shipper preflight --allow-new-crates
```

### CI integration (v0.2)

```bash
# Get GitHub Actions workflow
shipper ci github-actions

# Get GitLab CI workflow
shipper ci gitlab
```

### Resuming after interruption

```bash
# Resume normally
shipper resume

# Force resume if plan has changed
shipper resume --force-resume
```

### Custom output lines for evidence (v0.2)

```bash
# Capture more output lines for debugging
shipper publish --output-lines 100

# Capture fewer lines
shipper publish --output-lines 20
```

### Using verify modes (v0.2)

```bash
# Verify at workspace level (default)
shipper publish --verify-mode workspace

# Verify each package individually
shipper publish --verify-mode package

# Skip verification
shipper publish --verify-mode none
```

## CI templates

See `templates/` for example workflows.

## Documentation

- [Configuration](docs/configuration.md) - Configuration file options
- [Preflight Verification](docs/preflight.md) - Pre-flight verification guide
- [Readiness Checking](docs/readiness.md) - Readiness verification guide
- [Failure Modes](docs/failure-modes.md) - Common failure scenarios and solutions
- [Release Notes](RELEASE_NOTES_v0.2.0.md) - v0.2.0 release notes

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license

at your option.
