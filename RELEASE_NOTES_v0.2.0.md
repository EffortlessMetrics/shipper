# Shipper v0.2.0 Release Notes

## Overview

Shipper v0.2.0 is a major release that introduces four key pillars for reliable Rust crate publishing: **Evidence Capture**, **Event Logging**, **Readiness Checks**, and **Publish Policies**. These features provide enhanced reliability, better debugging capabilities, and more control over the publishing workflow.

This release significantly improves the publishing experience for teams working with multi-crate workspaces, with better support for CI/CD pipelines and comprehensive failure analysis.

## Summary of v0.2 Features

- **Evidence Capture** - Detailed stdout/stderr, exit codes, and timestamps for each operation
- **Event Logging** - Comprehensive event log (events.jsonl) for complete audit trails
- **Readiness Checks** - Configurable verification with API, index, and combined methods
- **Publish Policies** - Three built-in policies: safe, balanced, and fast
- **Preflight Verification** - Finishability assessment with Proven/NotProven/Failed states
- **Index-Based Readiness** - Direct sparse index verification for maximum accuracy
- **New Crate Detection** - Identifies first-time publishes during preflight
- **Schema Versioning** - State and receipt files include version information
- **Enhanced Receipts** - Git context, attempt evidence, and readiness evidence
- **CI Integration** - Built-in workflow generation for GitHub Actions and GitLab CI
- **Configuration File** - Project-specific settings via `.shipper.toml`

## Key Features

### 1. Evidence Capture

Every publish operation now captures detailed evidence for debugging and auditing:

- **Stdout/stderr output** from all commands
- **Exit codes** for precise failure classification
- **Timestamps** for timeline reconstruction
- **Command arguments** that were executed

Evidence is stored in both the receipt file and the event log, making it easy to investigate failures and understand exactly what happened during each step of the publishing process.

```bash
# View captured evidence
shipper inspect-receipt
```

### 2. Event Logging

A comprehensive event log (`events.jsonl`) records every step of the publishing process:

- Line-delimited JSON format for easy parsing
- Timestamps for each event
- Detailed context for each operation
- Complete audit trail for compliance

```bash
# View the complete event log
shipper inspect-events
```

### 3. Readiness Checks

Configurable readiness verification ensures published crates are actually available on the registry before proceeding:

- **API method** (fast) - Queries the registry API
- **Index method** (accurate) - Checks the crate index
- **Both method** (reliable) - Verifies using both methods

```bash
# Use index-based readiness
shipper publish --readiness-method index

# Use both methods for maximum reliability
shipper publish --readiness-method both
```

### 4. Publish Policies

Three built-in policies control verification behavior:

- **Safe** (default) - Verify every publish with strict checks
- **Balanced** - Verify only when needed
- **Fast** - Skip verification (use with caution)

```bash
# Choose a policy that fits your workflow
shipper publish --policy safe
shipper publish --policy balanced
shipper publish --policy fast
```

### 5. Preflight Verification

Comprehensive preflight checks run before any publishing begins:

- **Finishability Assessment** - Determines if workspace is ready (Proven/NotProven/Failed)
- **Ownership Verification** - Checks if you have permission to publish each crate
- **New Crate Detection** - Identifies crates that don't exist on registry yet
- **Workspace Dry-Run** - Verifies all packages can be published without uploading

```bash
# Run preflight checks
shipper preflight

# Run with strict ownership checks
shipper preflight --strict-ownership
```

### 6. Index-Based Readiness

Enhanced readiness verification with sparse index support:

- **Index Method** - Direct sparse index verification for maximum accuracy
- **Prefer Index** - When using both methods, prioritize index checks (via config file)
- **Custom Index Path** - Support for testing with custom index locations (via config file)

```bash
# Use index-based readiness
shipper publish --readiness-method index
```

### 7. Schema Versioning

State and receipt files now include version information:

- **State Version** - Identifies state file format version
- **Plan Version** - Identifies plan format version
- **Receipt Version** - Identifies receipt format version

This allows Shipper to handle format changes gracefully and provide clear migration paths.

### 8. Enhanced Receipts

Receipts now include comprehensive evidence:

- **Attempt Evidence** - Stdout/stderr, exit codes, and duration for each attempt
- **Readiness Evidence** - Timestamps and results of each readiness check
- **Git Context** - Optional git commit, branch, and tag information
- **Environment Fingerprint** - Shipper, Cargo, and Rust version information

```bash
# View detailed receipt with evidence
shipper inspect-receipt

# Get JSON output for CI integration
shipper inspect-receipt --format json
```

### 9. Configuration File Support

Project-specific configuration via `.shipper.toml`:

- Policy, verify mode, readiness, retry, and output settings
- Lock and parallel publishing configuration
- CLI flags always take precedence over config file values

```bash
# Generate a default configuration file
shipper config init

# Validate a configuration file
shipper config validate
```

### 10. Parallel Publishing

Publish packages concurrently when they have no dependency relationship:

- **Wave-based execution**: Packages at the same dependency level are published in parallel
- **Configurable concurrency**: Control max concurrent operations with `--max-concurrent`
- **Per-package timeouts**: Set individual package timeouts with `--per-package-timeout`

```bash
# Enable parallel publishing
shipper publish --parallel

# Limit concurrency
shipper publish --parallel --max-concurrent 2

# Set per-package timeout
shipper publish --parallel --per-package-timeout 10m
```

Or via configuration:

```toml
[parallel]
enabled = true
max_concurrent = 4
per_package_timeout = "30m"
```

## New CLI Commands

### Inspection Commands

- `shipper inspect-events` - View detailed event log with timestamps and evidence
- `shipper inspect-receipt` - View detailed receipt with captured evidence

### CI Commands

- `shipper ci github-actions` - Print GitHub Actions workflow snippet
- `shipper ci gitlab` - Print GitLab CI workflow snippet

### Configuration Commands

- `shipper config init` - Generate a default `.shipper.toml` configuration file
- `shipper config validate` - Validate a configuration file

### Cleanup Command

- `shipper clean` - Clean state files (state.json, receipt.json, events.jsonl)
  - `--keep-receipt` - Keep receipt.json while cleaning other files

## New CLI Flags

### Verification Options

- `--policy <policy>` - Publish policy: safe, balanced, or fast
- `--verify-mode <mode>` - Verify mode: workspace, package, or none
- `--no-verify` - Pass --no-verify to cargo publish

### Readiness Options

- `--readiness-method <method>` - Readiness check method: api, index, or both
- `--readiness-timeout <duration>` - How long to wait for registry visibility (default: 5m)
- `--readiness-poll <duration>` - Poll interval for readiness checks (default: 2s)
- `--no-readiness` - Disable readiness checks

### Evidence Options

- `--output-lines <number>` - Number of output lines to capture for evidence (default: 50)
- `--format <format>` - Output format: text or json

### Lock Options

- `--force` - Force override of existing locks
- `--lock-timeout <duration>` - Lock timeout duration (default: 1h)

### Configuration Options

- `--config <path>` - Path to a custom `.shipper.toml` configuration file

### Parallel Options

- `--parallel` - Enable parallel publishing of independent packages
- `--max-concurrent <N>` - Maximum number of concurrent publish operations
- `--per-package-timeout <duration>` - Timeout for each individual package publish

## Migration Guide from v0.1.0

### Step 1: Upgrade the version

Update your Cargo.toml or reinstall shipper:

```bash
cargo install --path crates/shipper-cli --locked
```

### Step 2: Clean old state files

The state file format has changed. Clean old state files before using v0.2:

```bash
shipper clean
```

### Step 3: Update CI workflows

Use the new CI command to generate updated workflow snippets:

```bash
# For GitHub Actions
shipper ci github-actions

# For GitLab CI
shipper ci gitlab
```

### Step 4: Review readiness settings

The default readiness timeout has increased from 2m to 5m for more reliable verification. Adjust if needed:

```bash
shipper publish --readiness-timeout 10m
```

### Step 5: Test publish policies

Try the different policy modes to find the best fit for your workflow:

```bash
# Start with safe mode (recommended)
shipper publish --policy safe

# Switch to balanced if safe mode is too slow
shipper publish --policy balanced
```

## Breaking Changes

### State File Format

The state file format has changed significantly. Previous versions of shipper cannot resume from v0.2 state files. You must run `shipper clean` before upgrading.

### Receipt File Format

The receipt file format has been enhanced with additional evidence fields. Tools that parse the receipt file may need to be updated.

### Default Readiness Timeout

The default readiness timeout has increased from 2m to 5m for more reliable verification. This may increase total publish time for large workspaces.

## Bug Fixes

- Fixed potential race conditions in state file handling
- Improved handling of ambiguous failures where upload may have succeeded
- Better error recovery for network timeouts
- Fixed issues with resume when workspace configuration changes

## Known Issues

### Registry API Rate Limits

Some registries may have aggressive rate limits that can cause publish failures even with backoff. If you encounter this, try:

```bash
shipper publish --max-attempts 10 --max-delay 5m
```

### Index-Based Readiness Performance

Index-based readiness checks can be slow for large registries. Consider using API-based readiness for faster publishes:

```bash
shipper publish --readiness-method api
```

### Token Scope Limitations

Some registry tokens may not allow querying ownership information. In this case, ownership checks will be skipped even with `--strict-ownership` enabled.

## Documentation

- [README.md](README.md) - Main documentation
- [CHANGELOG.md](CHANGELOG.md) - Detailed changelog
- [docs/configuration.md](docs/configuration.md) - Configuration file options
- [docs/preflight.md](docs/preflight.md) - Pre-flight verification guide
- [docs/readiness.md](docs/readiness.md) - Readiness verification guide
- [docs/failure-modes.md](docs/failure-modes.md) - Failure modes and debugging guide

---

**Version**: 0.2.0
**License**: MIT OR Apache-2.0
