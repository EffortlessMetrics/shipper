# Configuration File

Shipper supports project-specific configuration via a `.shipper.toml` file in your workspace root. This allows you to define publishing policies, readiness settings, and other options without passing CLI flags every time.

## Creating a Configuration File

Generate a default configuration file by running:

```bash
shipper config init
```

This creates a `.shipper.toml` file in your current directory with sensible defaults and inline documentation.

You can also specify a custom output path:

```bash
shipper config init -o my-config.toml
```

## Validating a Configuration File

Check if your configuration file is valid:

```bash
shipper config validate
```

Or validate a specific file:

```bash
shipper config validate -o my-config.toml
```

## Using a Configuration File

Shipper automatically looks for `.shipper.toml` in your workspace root. Place it alongside your `Cargo.toml` file.

You can also specify a custom configuration file using the `--config` flag:

```bash
shipper publish --config my-config.toml
```

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

### Preflight

```toml
[preflight]
# Skip owners/permissions preflight (not recommended)
skip_ownership_check = false
# Fail preflight if ownership checks fail (recommended)
strict_ownership = false
# Allow publishing new crates (first-time publishes)
allow_new_crates = true
# Require ownership verification for new crates (recommended)
require_ownership_for_new_crates = true
```

Preflight checks run before any publishing begins to verify your workspace is ready:

- **skip_ownership_check**: Skip checking if you have permission to publish to the registry. Not recommended for production.
- **strict_ownership**: Fail preflight immediately if ownership checks fail or if no token is available. Recommended for production.
- **allow_new_crates**: Allow publishing crates that don't exist on the registry yet. Set to `false` to prevent accidental new crate creation.
- **require_ownership_for_new_crates**: When publishing new crates, require ownership verification before proceeding. Recommended to prevent publishing to the wrong registry.

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
# Use index as primary method when Both is selected
prefer_index = false
# Custom index path for testing (optional)
index_path = "/path/to/custom/index"
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
| `prefer_index` | bool | `false` | When using `both`, prefer index over API |
| `index_path` | path | `None` | Custom index path for testing (optional) |

**Readiness Methods:**

- **api** (default): Check crates.io HTTP API. Fast and usually reliable.
- **index**: Check the sparse index. Slower but more accurate, as it directly verifies the crate index entry.
- **both**: Check both methods. Slowest but most reliable. Use `prefer_index` to prioritize index checks.

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
# Lock timeout duration (locks older than this are considered stale)
timeout = "1h"
```

Shipper uses a lock file to prevent concurrent publish operations. If a lock is older than the timeout, it's considered stale and can be overridden with `--force`.

### Retry

```toml
[retry]
# Max attempts per crate publish step
max_attempts = 6
# Base backoff delay
base_delay = "2s"
# Max backoff delay
max_delay = "2m"
```

Controls retry behavior for failed publish operations.

- **max_attempts**: Maximum number of retry attempts per package (default: `6`)
- **base_delay**: Starting delay for exponential backoff (default: `2s`)
- **max_delay**: Maximum delay between retries (default: `2m`)

### Flags

```toml
[flags]
# Allow publishing from a dirty git working tree (not recommended)
allow_dirty = false
```

- **allow_dirty**: Allow publishing even with uncommitted changes. Not recommended for production.

**Note**: Ownership and preflight settings have been moved to the `[preflight]` section. See the [Preflight](#preflight) section above.

### Registry

```toml
[registry]
name = "crates-io"
api_base = "https://crates.io"
```

Optional custom registry configuration. If not specified, defaults to crates.io.

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

[policy]
# Publishing policy: safe (verify+strict), balanced (verify when needed), or fast (no verify)
mode = "safe"

[verify]
# Verify mode: workspace (default, safest), package (per-crate), or none (no verify)
mode = "workspace"

[preflight]
# Skip owners/permissions preflight (not recommended)
skip_ownership_check = false
# Fail preflight if ownership checks fail (recommended)
strict_ownership = false
# Allow publishing new crates (first-time publishes)
allow_new_crates = true
# Require ownership verification for new crates (recommended)
require_ownership_for_new_crates = true

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
# Use index as primary method when Both is selected
prefer_index = false

[output]
# Number of output lines to capture for evidence
lines = 50

[lock]
# Lock timeout duration (locks older than this are considered stale)
timeout = "1h"

[retry]
# Max attempts per crate publish step
max_attempts = 6
# Base backoff delay
base_delay = "2s"
# Max backoff delay
max_delay = "2m"

[flags]
# Allow publishing from a dirty git working tree (not recommended)
allow_dirty = false

# Optional: Custom registry configuration
# [registry]
# name = "crates-io"
# api_base = "https://crates.io"
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
