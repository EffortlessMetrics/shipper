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

# Get JSON output for CI integration
shipper preflight --format json
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

### Package Status Columns

| Column | Meaning |
|--------|---------|
| `Published` | Whether the version already exists on the registry |
| `New Crate` | Whether the crate doesn't exist on the registry yet |
| `Auth Type` | Authentication method detected (`Token`, `Trusted`, `Unknown`, or `-`) |
| `Ownership` | Whether ownership was verified for this crate (`✓` or `✗`) |
| `Dry-run` | Whether the dry-run check passed for this crate (`✓` or `✗`) |

## Example Preflight Scenarios

### Scenario 1: Proven (Ready to Publish)

All checks pass. The "What to do next" section shows:

```
✓ All checks passed. Ready to publish with: shipper publish
```

**Next Step**: Run `shipper publish`

### Scenario 2: NotProven (No Token)

Token not detected, ownership can't be verified. The "What to do next" section shows:

```
⚠ Some checks could not be verified. You can still publish, but may encounter permission issues.
```

**Next Steps**:
1. Set `CARGO_REGISTRY_TOKEN` environment variable
2. Run `cargo login` to create credentials
3. Or proceed with `shipper publish` if you're confident

### Scenario 3: Failed (Ownership Not Verified)

Token detected but ownership check failed for one or more crates.

**Next Steps**:
1. Verify you're listed as an owner: `cargo owner --list my-crate`
2. Check your token has the correct scopes
3. Contact the crate owner to add you

### Scenario 4: Failed (Dry Run Failed)

The dry-run check failed, indicated by `✗` in the Dry-run column.

**Next Steps**:
1. Run `cargo publish --dry-run` manually to see the full error
2. Check for missing dependencies or version conflicts
3. Verify the package's `Cargo.toml` is valid

### Scenario 5: New Crate Detected

The New Crate column shows `Yes` for a package. This means the crate doesn't exist on the registry yet and will be created on first publish.

**Next Steps**:
1. Verify this is intentional: `cargo search new-crate`
2. Confirm you want to create a new crate on the registry
3. Proceed with `shipper publish`

## Configuration Options

Preflight behavior can be configured via `.shipper.toml` or CLI flags.

### Configuration File

Ownership and git-cleanliness settings live in the `[flags]` section:

```toml
[flags]
# Allow publishing from a dirty git working tree (not recommended)
allow_dirty = false
# Skip owners/permissions preflight (not recommended)
skip_ownership_check = false
# Fail preflight if ownership checks fail (recommended for production)
strict_ownership = false
```

### CLI Flags

```bash
# Skip ownership checks
shipper preflight --skip-ownership-check

# Fail preflight if ownership checks fail
shipper preflight --strict-ownership

# Allow dirty working tree
shipper preflight --allow-dirty
```

## Troubleshooting

### Issue: Preflight shows "NOT PROVEN" with no token

**Cause**: No registry token was found for ownership verification

**Solutions**:
1. Set `CARGO_REGISTRY_TOKEN` environment variable
2. Run `cargo login` to create credentials
3. Use `--skip-ownership-check` if you're confident (not recommended)

### Issue: Preflight shows "FAILED" for ownership

**Cause**: You don't have permission to publish the crate

**Solutions**:
1. Verify you're listed as an owner: `cargo owner --list <crate-name>`
2. Check your token has the correct scopes
3. Contact the crate owner to add you

### Issue: Preflight shows "FAILED" for dry run

**Cause**: The dry-run check failed, indicating issues with the package

**Solutions**:
1. Run `cargo publish --dry-run` manually to see the full error
2. Check for missing dependencies or version conflicts
3. Verify the package's `Cargo.toml` is valid

## Related Documentation

- [Configuration](configuration.md) - Configuration file options
- [Failure Modes](failure-modes.md) - Common failure scenarios and solutions
- [README](../README.md) - Main documentation
