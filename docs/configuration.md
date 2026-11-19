# Configuration File

Shipper supports project-specific configuration via a `.shipper.toml` file in your workspace root. This allows you to define publishing policies, readiness settings, and other options without passing CLI flags every time.

## Schema Versioning

Configuration files include a `schema_version` field to ensure compatibility with future versions of Shipper. The current version is `shipper.config.v1`.

```toml
schema_version = "shipper.config.v1"
```

## Creating a Configuration File

Generate a default configuration file by running:

```bash
shipper config init
```

This creates a new `.shipper.toml` file in your current directory with defaults
and inline documentation. An existing destination, including a symlink, causes
a nonzero refusal and is preserved. To keep an existing configuration and
generate a separate template, choose an unused output path:

```bash
shipper config init -o my-config.toml
```

The parent directory must already exist. The global `--force` option controls
release locks; it does not permit configuration overwrite.

## Validating a Configuration File

Check if your configuration file is valid:

```bash
shipper config validate
```

Or validate a specific file:

```bash
shipper config validate -p my-config.toml
```

## Using a Configuration File

Shipper automatically looks for `.shipper.toml` in your workspace root. Place it alongside your `Cargo.toml` file.

You can also specify a custom configuration file using the `--config` flag:

```bash
shipper publish --config my-config.toml
```

The effective registry is resolved before Shipper selects publishable packages
or computes the plan ID. Values in `[registry]` apply unless the corresponding
`--registry` or `--api-base` option overrides them. Configuration loaded from the
workspace root also applies when `--manifest-path` points to a member crate.
Equivalent configuration and CLI registry values produce the same plan.

For a registry catalog, select entries with `--registries NAME` or
`--all-registries`; declaring entries alone does not select them. `plan` and
`preflight` accept one effective target per invocation. Run them with
`--registries NAME` for each target before publishing to several registries.
Each publish/resume target uses its own registry-eligible package graph and
plan ID; multiple-target execution continues to keep state under each registry's
subdirectory.

## Configuration Options

### Policy

```toml
[policy]
# Publishing policy: safe (verify+strict), balanced (verify when needed), or fast (no verify)
mode = "safe"
```

- **safe** (default): Run verify with strict checks. Recommended for production.
- **balanced**: Run verify only when needed. Good balance of speed and safety.
- **fast**: No verify. Fastest option, but carries risk of publishing broken packages.

### Verify Mode

```toml
[verify]
# Verify mode: workspace (default, safest), package (per-crate), or none (no verify)
mode = "workspace"
```

- **workspace** (default): Run workspace dry-run to verify all packages. Safest option.
- **package**: Run verify per-crate during publish. Slower but more thorough.
- **none**: Skip verification. Not recommended.

### Readiness

```toml
[readiness]
# Enable readiness checks (wait for registry visibility after publish)
enabled = true
# Method for checking version visibility: api (fast), index (slower, more accurate), both (slowest, most reliable)
method = "api"
# Initial delay before first poll
initial_delay = "1s"
# Maximum delay between polls
max_delay = "60s"
# Maximum total time to wait for visibility
max_total_wait = "5m"
# Base poll interval
poll_interval = "2s"
# Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter)
jitter_factor = 0.5
# Use index as primary method when Both is selected (config-only, no CLI flag)
prefer_index = false
```

Readiness checks ensure your published packages are visible on the registry before continuing. This is important for workspaces where later packages depend on earlier ones.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | bool | `true` | Enable readiness checks |
| `method` | enum | `api` | Method to use: `api`, `index`, or `both` |
| `initial_delay` | duration | `1s` | Time to wait before first visibility check |
| `max_delay` | duration | `60s` | Maximum delay between polls with exponential backoff |
| `max_total_wait` | duration | `5m` | Maximum total time to wait for visibility |
| `poll_interval` | duration | `2s` | Base interval between polls |
| `jitter_factor` | float | `0.5` | Randomization factor for delays (0.0 = no jitter, 1.0 = full jitter) |
| `prefer_index` | bool | `false` | When using `both`, prefer index over API (config-only) |
| `index_path` | path | `None` | Custom index path for testing (config-only, optional) |

**Readiness Methods:**

- **api** (default): Check crates.io HTTP API. Fast and usually reliable.
- **index**: Check the sparse index. Slower but more accurate, as it directly verifies the crate index entry.
- **both**: Check both methods. Slowest but most reliable. Use `prefer_index` to prioritize index checks.

> **Note:** `prefer_index` and `index_path` are config-file-only settings with no corresponding CLI flags.

### Output

```toml
[output]
# Number of output lines to capture for evidence
lines = 50
```

Controls how many lines of stdout/stderr are captured for each publish attempt. This is included in the receipt for debugging.

### Lock

```toml
[lock]
# Minimum age for confirmed abandoned legacy lock recovery
timeout = "1h"
```

Shipper holds an exclusive operating-system guard for cooperating publishers
and cleanup commands. Its sidecar (`lock.guard`, or `lock_<workspace-hash>.guard`)
is persistent; do not delete it to force acquisition. The JSON lock record
remains readable holder metadata, and its age does not establish ownership.

After acquiring the guard, Shipper can recover an abandoned same-host record
from the current ownership protocol. Legacy records additionally need positive
same-host, same-process-scope evidence that the holder is absent and an age
strictly greater than `timeout`; a future timestamp never proves abandonment.
Corrupt, foreign, and otherwise unknown records are preserved by default.

`--force` may override unknown metadata only after winning the guard. It assumes
any noncooperating legacy or foreign publisher is quiescent. It cannot steal an
active OS guard or a positively observed live legacy holder, and does not
bypass retained-state admission or authorize another upload. `shipper clean`
holds the same guard throughout deletion and leaves the guard sidecar in place.
These guarantees require cooperating processes using the same lock key on a
filesystem with supported OS locking; they do not establish distributed or
network-filesystem exclusion.

### Retry

```toml
[retry]
# Retry policy preset: default, aggressive, conservative, or custom
policy = "default"
# Retry strategy: immediate, exponential (default), linear, constant
strategy = "exponential"
# Cumulative max attempts per crate across publish and resume
max_attempts = 6
# Base backoff delay
base_delay = "2s"
# Max backoff delay
max_delay = "2m"
# Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter)
jitter = 0.5
```

Controls retry behavior for failed publish operations.

- **policy**: Retry policy preset (default: `default`). Options: `default`, `aggressive`, `conservative`, `custom`
- **strategy**: Retry strategy (default: `exponential`). Options: `immediate`, `exponential`, `linear`, `constant`
- **max_attempts**: Maximum number of retry attempts per package (default: `6`)
- **base_delay**: Starting delay for exponential backoff (default: `2s`)
- **max_delay**: Maximum delay between retries (default: `2m`)
- **jitter**: Jitter factor for randomized delays (default: `0.5`)

`retry.per_error` class policies are unsupported. Earlier versions accepted
them without changing execution; remove every `retryable`, `ambiguous`, or
`permanent` class entry and use the global `[retry]` settings instead. `config
validate`, validated configuration loading, and public engine execution entry
points refuse configured classes before execution effects, even when CLI retry
flags or force flags are supplied. An omitted value or empty `[retry.per_error]`
table is allowed; an empty class subtable still configures that class and is
refused. Unknown keys within `per_error` fail decoding. Diagnostic-only loaders
can still inspect known class records, but those records cannot be executed.
See [failure modes](failure-modes.md#global-retry-configuration) for migration and
the unchanged retry safety rules.

### Flags

```toml
[flags]
# Allow publishing from a dirty git working tree (not recommended)
allow_dirty = false
# Skip owners/permissions preflight (not recommended)
skip_ownership_check = false
# Require a token during strict preflight and before publish execution
strict_ownership = false
```

- **allow_dirty**: Allow publishing even with uncommitted changes. Not recommended for production.
- **skip_ownership_check**: Skip checking if you have permission to publish to the registry. Not recommended for production.
- **strict_ownership**: Require a registry token during preflight and before publish execution. Preflight also fails when ownership verification fails; publish checks every selected registry before creating execution state but does not repeat preflight's registry ownership query. Recommended for production.

### Parallel

```toml
[parallel]
# Enable parallel publishing (default: false for sequential)
enabled = false
# Maximum number of concurrent publish operations (default: 4)
max_concurrent = 4
# Timeout per package publish operation (default: 30 minutes)
per_package_timeout = "30m"
```

Controls parallel publishing behavior. When enabled, packages at the same dependency level can be published concurrently.

- **enabled**: Enable parallel publishing (default: `false`, sequential publishing)
- **max_concurrent**: Maximum number of concurrent publish operations (default: `4`)
- **per_package_timeout**: Timeout for each individual package publish (default: `30m`)

The per-package Cargo timeout applies in both sequential and parallel mode;
the setting remains in `[parallel]` for configuration compatibility. Shipper
drains stdout and stderr while Cargo runs, so verbose output does not block
the child on a full pipe. The configured line tails are sanitized before
being retained in events and receipts.

After direct-child exit, timeout, or controlled interruption, Linux, macOS,
and Windows allow up to one additional second to confirm owned-process cleanup
and drain both output streams. Windows uses one Job Object per invocation.
Shipper retains a cleanup hold when it cannot confirm cleanup. A timed-out
publish requires registry reconciliation before Shipper can consider another
upload. See [#412](https://github.com/EffortlessMetrics/shipper-swarm/issues/412)
for platform qualification.

### Registry

```toml
[registry]
name = "crates-io"
api_base = "https://crates.io"
index_base = "https://index.crates.io"
# Custom registries must declare their sparse index explicitly.
# allow_private = false
# Loopback HTTP is disabled by default. Enable it only for a named local test
# registry through the explicit [rehearsal] posture below.
```

Optional custom registry configuration. If not specified, defaults to crates.io.
Registry API and index URLs must use HTTPS. URLs with userinfo, queries, or
fragments are rejected. Literal private-network destinations require the
explicit `allow_private = true` opt-in; metadata and link-local destinations
remain rejected. Plain HTTP is reserved for a configured local test registry
whose `[rehearsal]` section identifies that registry and sets
`allow_loopback = true`; `rehearsal.enabled = true` also grants that named
registry the explicit rehearsal posture.
For command-line-only local fixtures, `--allow-loopback` provides the same
explicit loopback posture for the selected registry; it never enables private
network destinations or the live-publish rehearsal gate.

The validated registry boundary captures credential authority separately from
display URLs. Authenticated requests are sent only to the validated authority;
authority-changing redirects are rejected or revalidated without forwarding
authorization. DNS-resolved IPv4/IPv6 destinations are classified as well as
their textual hostnames. A private registry remains supported, but only when
its operator explicitly opts into the private-network policy.

## CLI Override

CLI flags always take precedence over configuration file values. For example:

```toml
# .shipper.toml
[policy]
mode = "safe"
```

```bash
shipper publish --policy fast
```

The `--policy fast` flag will override the config file and use `fast` mode.

## Example Configuration

```toml
# Shipper configuration file
# This file should be placed in your workspace root as .shipper.toml

# Schema version
schema_version = "shipper.config.v1"

[policy]
# Publishing policy: safe (verify+strict), balanced (verify when needed), or fast (no verify)
mode = "safe"

[verify]
# Verify mode: workspace (default, safest), package (per-crate), or none (no verify)
mode = "workspace"

[readiness]
# Enable readiness checks (wait for registry visibility after publish)
enabled = true
# Method for checking version visibility: api (fast), index (slower, more accurate), both (slowest, most reliable)
method = "api"
# Initial delay before first poll
initial_delay = "1s"
# Maximum delay between polls
max_delay = "60s"
# Maximum total time to wait for visibility
max_total_wait = "5m"
# Base poll interval
poll_interval = "2s"
# Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter)
jitter_factor = 0.5

[output]
# Number of output lines to capture for evidence
lines = 50

[lock]
# Minimum age for confirmed abandoned legacy lock recovery
timeout = "1h"

[retry]
# Retry policy preset: default, aggressive, conservative, or custom
policy = "default"
# Retry strategy: immediate, exponential (default), linear, constant
strategy = "exponential"
# Cumulative max attempts per crate across publish and resume
max_attempts = 6
# Base backoff delay
base_delay = "2s"
# Max backoff delay
max_delay = "2m"
# Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter)
jitter = 0.5

[flags]
# Allow publishing from a dirty git working tree (not recommended)
allow_dirty = false
# Skip owners/permissions preflight (not recommended)
skip_ownership_check = false
# Require a token during strict preflight and before publish execution
strict_ownership = false

[parallel]
# Enable parallel publishing (default: false for sequential)
enabled = false
# Maximum number of concurrent publish operations (default: 4)
max_concurrent = 4
# Timeout per package publish operation (default: 30 minutes)
per_package_timeout = "30m"

# Optional: Custom registry configuration
# [registry]
# name = "crates-io"
# api_base = "https://crates.io"
# index_base = "https://index.crates.io"
```

## Migration from CLI Flags

If you're currently using CLI flags, here's how to migrate to a configuration file:

```bash
# Before (CLI only)
shipper publish --policy fast --max-attempts 3 --no-readiness

# After (with config file)
# .shipper.toml:
# [policy]
# mode = "fast"
# [retry]
# max_attempts = 3
# [readiness]
# enabled = false

shipper publish
```

## Backward Compatibility

Configuration file support is fully backward compatible. If no `.shipper.toml` file exists, Shipper works exactly as before using CLI flags and defaults.
