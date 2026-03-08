# Preflight Verification

Preflight verification is a safety check that runs before any publishing begins. It assesses whether your workspace is ready to publish and identifies potential issues early, before any crates are uploaded to the registry.

## Running Preflight

```bash
# Run preflight checks
shipper preflight

# Run with strict ownership checks
shipper preflight --strict-ownership

# Run with ownership checks skipped
shipper preflight --skip-ownership-check

# Allow dirty working tree
shipper preflight --allow-dirty

# Skip dry-run verification
shipper preflight --no-verify

# Use per-package verification instead of workspace-level
shipper preflight --verify-mode package

# Get JSON output for CI integration
shipper preflight --format json

# Combine flags
shipper preflight --strict-ownership --format json --quiet
```

## Complete List of Checks

Preflight runs the following checks in order. Each check can pass, warn, or fail depending on the result and your configuration.

### 1. Git Cleanliness

Runs `git status --porcelain` against the workspace root. If the working tree has uncommitted changes, preflight fails immediately.

**Controlled by:** `--allow-dirty` flag or `[flags] allow_dirty` in config.

**Error on failure:**

```
Error: git working tree is not clean; commit/stash changes or use --allow-dirty
```

**Skipped when:** `--allow-dirty` is set, or `[flags] allow_dirty = true` in `.shipper.toml`.

### 2. Registry Reachability

Initializes an HTTP client for the target registry (default: `https://crates.io`) and verifies it can connect. This is tested implicitly by the version existence checks that follow.

**Error on failure:**

```
Error: registry request failed
```

### 3. Token Detection & Authentication

Resolves a registry token using Cargo's standard resolution order:

1. `CARGO_REGISTRY_TOKEN` environment variable (for crates.io)
2. `CARGO_REGISTRIES_<NAME>_TOKEN` environment variable (for alternative registries)
3. `$CARGO_HOME/credentials.toml` (created by `cargo login`)
4. `$CARGO_HOME/credentials` (legacy format)

Also detects authentication type:

| Auth Type | Meaning |
|-----------|---------|
| `Token` | A Cargo registry token was found |
| `Trusted` | GitHub Actions OIDC trusted publishing environment detected |
| `Unknown` | Partial OIDC environment (only one of `ACTIONS_ID_TOKEN_REQUEST_URL` / `ACTIONS_ID_TOKEN_REQUEST_TOKEN` set) |
| `-` | No authentication found |

**Error on failure (strict mode only):**

```
Error: strict ownership requested but no token found (set CARGO_REGISTRY_TOKEN or run cargo login)
```

### 4. Dry-Run Verification

Runs `cargo publish --dry-run` to verify all packages compile and pass packaging checks. The scope depends on the verify mode:

| Verify Mode | Behavior |
|-------------|----------|
| `workspace` (default) | Runs `cargo publish --workspace --dry-run` once for the entire workspace |
| `package` | Runs `cargo publish -p <name> --dry-run` individually for each package |
| `none` | Skips dry-run entirely |

**Controlled by:** `--no-verify` flag, `--verify-mode` flag, `--policy` flag, or `[verify] mode` / `[policy] mode` in config.

**Skipped when:**
- `--no-verify` is set
- `--verify-mode none` is set
- `--policy fast` is set (disables all verification)

**Error output on failure:**

```
Dry-run Failures:
-----------------
Package: my-crate@0.2.0
exit_code=101; stdout_tail=["..."]; stderr_tail=["error[E0433]: failed to resolve..."]
```

### 5. Version Existence Check

For each package, queries the registry API (`GET /api/v1/crates/<name>/<version>`) to determine if the version is already published. Already-published packages are flagged in the report.

**Error on failure:**

```
Error: unexpected status while checking version existence: 500 Internal Server Error
```

### 6. New Crate Detection

For each package, queries the registry API (`GET /api/v1/crates/<name>`) to check whether the crate exists. Crates that don't exist yet are flagged as `New Crate: Yes` in the report and recorded in the event log.

**Error on failure:**

```
Error: unexpected status while checking crate existence: 500 Internal Server Error
```

### 7. Ownership Verification

For each existing (non-new) crate, queries the registry owners endpoint (`GET /api/v1/crates/<name>/owners`) to verify your token has publish permissions. Behavior depends on the ownership mode:

| Mode | Behavior on failure |
|------|---------------------|
| Default (non-strict) | Logs a warning, sets ownership to unverified, continues |
| `--strict-ownership` | Fails preflight immediately with an error |
| `--skip-ownership-check` | Skips the check entirely |

New crates are always skipped for ownership checks (they have no owners endpoint).

**Warning (non-strict mode):**

```
owners preflight failed for my-crate; continuing (non-strict mode)
```

**Error (strict mode, API failure):**

```
Error: forbidden when querying owners; token may be invalid or missing required scope
```

```
Error: crate not found when querying owners: my-crate
```

## CLI Flags Reference

All flags are global (work with any subcommand, including `preflight` and `publish`).

| Flag | Description | Default |
|------|-------------|---------|
| `--allow-dirty` | Allow publishing from a dirty git working tree | `false` |
| `--skip-ownership-check` | Skip the owners/permissions preflight check | `false` |
| `--strict-ownership` | Fail preflight if ownership checks fail or no token is available | `false` |
| `--no-verify` | Pass `--no-verify` to `cargo publish` (skips dry-run) | `false` |
| `--verify-mode <MODE>` | Dry-run scope: `workspace` (default), `package`, or `none` | `workspace` |
| `--policy <POLICY>` | Publish policy preset (see below) | `safe` |
| `--format <FORMAT>` | Output format: `text` or `json` | `text` |
| `--quiet` / `-q` | Suppress informational output | `false` |
| `--api-base <URL>` | Registry API base URL | `https://crates.io` |
| `--package <NAME>` | Restrict to specific packages (repeatable) | all packages |
| `--state-dir <PATH>` | Directory for state and receipts | `.shipper` |

### Policy Presets

The `--policy` flag provides presets that control multiple checks at once:

| Policy | Dry-run | Ownership Check | Strict Ownership | Readiness |
|--------|---------|-----------------|------------------|-----------|
| `safe` (default) | ✓ (unless `--no-verify`) | ✓ (unless `--skip-ownership-check`) | Respects `--strict-ownership` | ✓ |
| `balanced` | ✓ (unless `--no-verify`) | ✗ | ✗ | ✓ |
| `fast` | ✗ | ✗ | ✗ | ✗ |

## Configuration Options

Preflight behavior can be configured via `.shipper.toml` or CLI flags. CLI flags always take precedence over config file values.

### `[flags]` Section

Controls git-cleanliness and ownership behavior:

```toml
[flags]
# Allow publishing from a dirty git working tree (default: false)
allow_dirty = false

# Skip owners/permissions preflight (default: false)
skip_ownership_check = false

# Fail preflight if ownership checks fail (default: false)
strict_ownership = false
```

Merge rule: CLI flags are OR-merged with config values. Setting `allow_dirty = true` in config or passing `--allow-dirty` on the command line both enable the flag.

### `[policy]` Section

Sets the publish policy preset:

```toml
[policy]
# Options: safe (default), balanced, fast
mode = "safe"
```

### `[verify]` Section

Controls dry-run verification:

```toml
[verify]
# Options: workspace (default), package, none
mode = "workspace"
```

### Complete Example

```toml
[policy]
mode = "safe"

[verify]
mode = "workspace"

[flags]
allow_dirty = false
skip_ownership_check = false
strict_ownership = true
```

## Finishability Assessment

Preflight produces one of three finishability states:

### Proven

All checks passed — dry-runs succeeded and ownership was verified for every package. Your workspace is ready to publish.

**Action:** Proceed with `shipper publish`.

### NotProven

Dry-runs passed, but ownership could not be verified for one or more packages. This typically happens when no token is available, the token lacks the required scope, or the ownership API returned an error in non-strict mode.

**Action:** Review the warnings and proceed if you're confident, or provide a token and run preflight again.

### Failed

A critical check failed. Either a dry-run failed (any package), or `--strict-ownership` was set and ownership verification failed.

**Action:** Fix the issues identified in the report before publishing.

## Interpreting Preflight Output

### Text Output

Preflight outputs a table-based report showing each package's status:

```
Preflight Report
===============

Plan ID: plan-abc123
Timestamp: 2025-02-10T15:30:00Z

Token Detected: ✓

Finishability: PROVEN

Packages:
┌─────────────────────┬─────────┬──────────┬──────────┬───────────────┬─────────────┬─────────────┐
│ Package             │ Version │ Published│ New Crate │ Auth Type     │ Ownership   │ Dry-run     │
├─────────────────────┼─────────┼──────────┼──────────┼───────────────┼─────────────┼─────────────┤
│ my-core             │ 0.2.0   │ No       │ No       │ Token         │ ✓           │ ✓           │
│ my-utils            │ 0.2.0   │ No       │ Yes      │ Token         │ ✓           │ ✓           │
└─────────────────────┴─────────┴──────────┴──────────┴───────────────┴─────────────┴─────────────┘

Summary:
  Total packages: 2
  Already published: 0
  New crates: 1
  Ownership verified: 2
  Dry-run passed: 2

What to do next:
-----------------
✓ All checks passed. Ready to publish with: shipper publish
```

### JSON Output

Use `--format json` to get machine-readable output for CI integration:

```bash
shipper preflight --format json
```

The JSON output contains the full `PreflightReport` structure including `plan_id`, `token_detected`, `finishability`, `timestamp`, and a `packages` array with per-package results.

### Package Status Columns

| Column | Meaning |
|--------|---------|
| `Published` | Whether the version already exists on the registry |
| `New Crate` | Whether the crate doesn't exist on the registry yet |
| `Auth Type` | Authentication method detected (`Token`, `Trusted`, `Unknown`, or `-`) |
| `Ownership` | Whether ownership was verified for this crate (`✓` or `✗`) |
| `Dry-run` | Whether the dry-run check passed for this crate (`✓` or `✗`) |

## CI Integration

### Recommended CI Pattern

Run `shipper preflight` as a separate step before `shipper publish` to catch issues early and get a clear report:

```yaml
# GitHub Actions example
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable

      - name: Install shipper
        run: cargo install shipper-cli --locked

      - name: Preflight checks
        run: shipper preflight --strict-ownership --format json
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}

      - name: Publish
        run: shipper publish --quiet
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}

      - name: Upload receipts
        if: always()
        uses: actions/upload-artifact@v6
        with:
          name: shipper-state
          path: .shipper/
```

### Using JSON Output in CI

Parse the JSON output to make decisions in your pipeline:

```yaml
- name: Run preflight
  id: preflight
  run: |
    shipper preflight --format json > preflight.json
    FINISHABILITY=$(jq -r '.finishability' preflight.json)
    echo "finishability=$FINISHABILITY" >> "$GITHUB_OUTPUT"
  env:
    CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}

- name: Gate on preflight
  if: steps.preflight.outputs.finishability != 'Proven'
  run: |
    echo "Preflight not proven, review preflight.json"
    cat preflight.json | jq .
    exit 1
```

### Recommended CI Flags

| Scenario | Flags |
|----------|-------|
| Production release | `--strict-ownership` |
| Pre-release validation (no publish) | `shipper preflight --format json` |
| Fast CI (skip verification) | `--policy fast` |
| Trusted publishing (GitHub OIDC) | No extra flags needed — auth is auto-detected |
| Dirty working tree in CI | `--allow-dirty` (e.g., when CI modifies files) |

### Event Log

Preflight writes structured events to `.shipper/events.jsonl` in the state directory. Events emitted during preflight include:

- `PreflightStarted` — preflight began
- `PreflightWorkspaceVerify` — workspace dry-run result (passed/failed with output)
- `PreflightNewCrateDetected` — a crate was identified as new (not yet on the registry)
- `PreflightOwnershipCheck` — per-crate ownership verification result
- `PreflightComplete` — preflight finished with finishability assessment

These events can be used for auditing and debugging CI failures.

## Example Preflight Scenarios

### Scenario 1: Proven (Ready to Publish)

All checks pass. The "What to do next" section shows:

```
✓ All checks passed. Ready to publish with: shipper publish
```

**Next Step:** Run `shipper publish`.

### Scenario 2: NotProven (No Token)

Token not detected, ownership can't be verified:

```
Token Detected: ✗
Finishability: NOT PROVEN

⚠ Some checks could not be verified. You can still publish, but may encounter permission issues.
```

**Next Steps:**
1. Set `CARGO_REGISTRY_TOKEN` environment variable
2. Run `cargo login` to create credentials
3. Or proceed with `shipper publish` if you're confident

### Scenario 3: Failed (Strict Ownership, No Token)

`--strict-ownership` was set but no token was available. Preflight exits with an error before the report is printed:

```
Error: strict ownership requested but no token found (set CARGO_REGISTRY_TOKEN or run cargo login)
```

### Scenario 4: Failed (Ownership Check Failed, Strict Mode)

Token detected but the owners API returned a 403:

```
Error: forbidden when querying owners; token may be invalid or missing required scope
```

**Next Steps:**
1. Verify you're listed as an owner: `cargo owner --list <crate-name>`
2. Check your token has the correct scopes
3. Contact the crate owner to add you

### Scenario 5: Failed (Dry Run Failed)

The dry-run check failed. The report shows `✗` in the Dry-run column and includes failure details:

```
Dry-run Failures:
-----------------
Package: my-crate@0.2.0
exit_code=101; stdout_tail=["..."]; stderr_tail=["error[E0433]: failed to resolve..."]

✗ Preflight failed. Please fix the issues above before publishing.
```

**Next Steps:**
1. Run `cargo publish --dry-run -p my-crate` manually to see the full error
2. Check for missing dependencies or version conflicts
3. Verify the package's `Cargo.toml` is valid

### Scenario 6: New Crate Detected

The New Crate column shows `Yes` for a package. This means the crate doesn't exist on the registry yet and will be created on first publish.

**Next Steps:**
1. Verify this is intentional: `cargo search new-crate`
2. Confirm you want to create a new crate on the registry
3. Proceed with `shipper publish`

## Troubleshooting

### Preflight shows "NOT PROVEN" with no token

**Cause:** No registry token was found for ownership verification.

**Solutions:**
1. Set `CARGO_REGISTRY_TOKEN` environment variable
2. Run `cargo login` to create credentials
3. Use `--skip-ownership-check` if you're confident (not recommended)

### Preflight shows "FAILED" for ownership

**Cause:** You don't have permission to publish the crate.

**Solutions:**
1. Verify you're listed as an owner: `cargo owner --list <crate-name>`
2. Check your token has the correct scopes
3. Contact the crate owner to add you

### Preflight shows "FAILED" for dry run

**Cause:** The dry-run check failed, indicating issues with the package.

**Solutions:**
1. Run `cargo publish --dry-run` manually to see the full error
2. Check for missing dependencies or version conflicts
3. Verify the package's `Cargo.toml` is valid

### `git status failed` error

**Cause:** Git is not installed or the workspace is not a git repository.

**Solutions:**
1. Install git and ensure it's in your `PATH`
2. Initialize a git repository: `git init`
3. Use `--allow-dirty` to skip the git check

## Related Documentation

- [Configuration](configuration.md) — Full `.shipper.toml` reference
- [Failure Modes](failure-modes.md) — Common failure scenarios and recovery
- [Readiness](readiness.md) — Post-publish registry visibility verification
- [README](../README.md) — Quick start and installation
