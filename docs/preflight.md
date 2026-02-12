# Preflight Verification

Preflight verification is a safety check that runs before any publishing begins. It assesses whether your workspace is ready to publish and identifies potential issues early, before any crates are uploaded to the registry.

## What Preflight Does

Preflight performs the following checks:

1. **Git Cleanliness** - Verifies the working tree is clean (unless `--allow-dirty` is set)
2. **Registry Reachability** - Checks that the registry is accessible
3. **Version Existence** - Verifies the target versions don't already exist on the registry
4. **Workspace Dry-Run** - Runs `cargo publish --dry-run` to verify all packages can be published
5. **Ownership Verification** - Checks if you have permission to publish each crate (when a token is available)
6. **New Crate Detection** - Identifies crates that don't exist on the registry yet

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
```

## Finishability Assessment

Preflight produces one of three finishability states:

### Proven

All checks passed. Your workspace is ready to publish.

**Action**: Proceed with `shipper publish`

### NotProven

Some checks couldn't be verified, typically because no token was available for ownership checks. The workspace may be publishable, but couldn't be fully verified.

**Action**: Review the warnings and proceed if you're confident, or provide a token and run preflight again.

### Failed

Critical checks failed. Publishing would likely fail.

**Action**: Fix the issues before publishing.

## Interpreting Preflight Output

Preflight outputs a detailed report for each package:

```
Preflight Report
===============

Plan ID: plan-abc123
Token Detected: true
Finishability: Proven

Packages:
---------

my-crate@0.1.0
  Already Published: false
  Is New Crate: true
  Auth Type: Token
  Ownership Verified: ✓
  Dry Run Passed: ✓

dependency-crate@0.2.0
  Already Published: false
  Is New Crate: false
  Auth Type: Token
  Ownership Verified: ✓
  Dry Run Passed: ✓
```

### Package Status Indicators

| Field | Meaning |
|-------|---------|
| `Already Published` | The version already exists on the registry |
| `Is New Crate` | The crate doesn't exist on the registry yet |
| `Auth Type` | Authentication method detected (`Token`, `TrustedPublishing`, or `Unknown`) |
| `Ownership Verified` | Whether ownership was verified for this crate |
| `Dry Run Passed` | Whether the dry-run check passed for this crate |

## Example Preflight Outputs

### Example 1: Proven (Ready to Publish)

```
Preflight Report
===============

Plan ID: plan-abc123
Token Detected: true
Finishability: Proven

Packages:
---------

my-crate@0.1.0
  Already Published: false
  Is New Crate: false
  Auth Type: Token
  Ownership Verified: ✓
  Dry Run Passed: ✓
```

**Interpretation**: All checks passed. Ready to publish.

**Next Step**: Run `shipper publish`

### Example 2: NotProven (No Token)

```
Preflight Report
===============

Plan ID: plan-abc123
Token Detected: false
Finishability: NotProven

Packages:
---------

my-crate@0.1.0
  Already Published: false
  Is New Crate: false
  Auth Type: Unknown
  Ownership Verified: ✗ (no token)
  Dry Run Passed: ✓
```

**Interpretation**: Workspace verified, but ownership couldn't be checked because no token was available.

**Next Steps**:
1. Set `CARGO_REGISTRY_TOKEN` environment variable
2. Run `cargo login` to create credentials
3. Or proceed with `shipper publish` if you're confident

### Example 3: Failed (Ownership Not Verified)

```
Preflight Report
===============

Plan ID: plan-abc123
Token Detected: true
Finishability: Failed

Packages:
---------

my-crate@0.1.0
  Already Published: false
  Is New Crate: false
  Auth Type: Token
  Ownership Verified: ✗
  Dry Run Passed: ✓
```

**Interpretation**: Ownership check failed. You don't have permission to publish this crate.

**Next Steps**:
1. Verify you're listed as an owner: `cargo owner --list my-crate`
2. Check your token has the correct scopes
3. Contact the crate owner to add you

### Example 4: Failed (Dry Run Failed)

```
Preflight Report
===============

Plan ID: plan-abc123
Token Detected: true
Finishability: Failed

Packages:
---------

my-crate@0.1.0
  Already Published: false
  Is New Crate: false
  Auth Type: Token
  Ownership Verified: ✓
  Dry Run Passed: ✗
```

**Interpretation**: The dry-run check failed, indicating issues with the package.

**Next Steps**:
1. Run `cargo publish --dry-run` manually to see the full error
2. Check for missing dependencies or version conflicts
3. Verify the package's `Cargo.toml` is valid

### Example 5: New Crate Detected

```
Preflight Report
===============

Plan ID: plan-abc123
Token Detected: true
Finishability: Proven

Packages:
---------

new-crate@0.1.0
  Already Published: false
  Is New Crate: true
  Auth Type: Token
  Ownership Verified: ✓
  Dry Run Passed: ✓
```

**Interpretation**: This crate doesn't exist on the registry yet. This is a new crate publish.

**Next Steps**:
1. Verify this is intentional: `cargo search new-crate`
2. Confirm you want to create a new crate on the registry
3. Proceed with `shipper publish`

## Configuration Options

Preflight behavior can be configured via `.shipper.toml` or CLI flags:

### Configuration File

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

### CLI Flags

```bash
# Skip ownership checks
shipper preflight --skip-ownership-check

# Fail preflight if ownership checks fail
shipper preflight --strict-ownership

# Allow dirty working tree
shipper preflight --allow-dirty

# Prevent new crate publishing
shipper preflight --no-allow-new-crates
```

## Troubleshooting

### Issue: Preflight shows "NotProven" with no token

**Cause**: No registry token was found for ownership verification

**Solutions**:
1. Set `CARGO_REGISTRY_TOKEN` environment variable
2. Run `cargo login` to create credentials
3. Use `--skip-ownership-check` if you're confident (not recommended)

### Issue: Preflight shows "Failed" for ownership

**Cause**: You don't have permission to publish the crate

**Solutions**:
1. Verify you're listed as an owner: `cargo owner --list <crate-name>`
2. Check your token has the correct scopes
3. Contact the crate owner to add you

### Issue: Preflight shows "Failed" for dry run

**Cause**: The dry-run check failed, indicating issues with the package

**Solutions**:
1. Run `cargo publish --dry-run` manually to see the full error
2. Check for missing dependencies or version conflicts
3. Verify the package's `Cargo.toml` is valid

### Issue: Preflight shows new crate detected but not allowed

**Cause**: A crate doesn't exist on the registry yet, but `allow_new_crates` is disabled

**Solutions**:
1. Verify this is intentional: `cargo search <crate-name>`
2. Enable new crate publishing: `shipper preflight --allow-new-crates`
3. Or set in config: `preflight.allow_new_crates = true`

## Best Practices

1. **Always run preflight before publishing** - This catches issues early before any crates are uploaded
2. **Use strict ownership checks in production** - This ensures you have permission to publish before attempting
3. **Review NotProven status carefully** - If preflight can't verify ownership, make sure you're confident before proceeding
4. **Keep your working tree clean** - Publishing from a dirty tree can lead to unexpected results
5. **Check for new crates intentionally** - New crate creation is a significant action, verify it's intended

## Related Documentation

- [Configuration](configuration.md) - Configuration file options
- [Failure Modes](failure-modes.md) - Common failure scenarios and solutions
- [README](../README.md) - Main documentation
