# Failure modes and how shipper handles them

This tool treats publishing as an **irreversible, non-atomic workflow**.

## Partial publishes

If a workspace publish is interrupted after some crates have been uploaded, re-running `shipper publish` (or `shipper resume`) should skip already-published versions and continue with the remainder.

Shipper maintains a persistent state file (`.shipper/state.json`) that tracks which crates have been successfully published. When resuming, Shipper:

1. Reads the previous state
2. Validates the state against the current workspace configuration
3. Skips crates that were already successfully published
4. Continues with the remaining crates

## Ambiguous timeouts

Cargo may fail locally even when the upload succeeded server-side. Shipper verifies the registry state (`crate@version exists`) before treating a step as failed.

### Evidence capture (v0.2)

When failures occur, Shipper now captures detailed evidence:

- **Stdout/stderr output** from the failed command
- **Exit codes** for precise failure classification
- **Timestamps** for timeline reconstruction
- **Command arguments** that were executed

This evidence is stored in:
- `.shipper/events.jsonl` - Line-delimited event log
- `.shipper/receipt.json` - Structured receipt with embedded evidence

### Inspecting failures (v0.2)

Use the inspection commands to debug failures:

```bash
# View the complete event log
shipper inspect-events

# View the receipt with captured evidence
shipper inspect-receipt

# Get JSON output for automated analysis
shipper inspect-receipt --format json
```

The event log shows a chronological record of all operations, including:
- Preflight checks and their results
- Each publish attempt with retry details
- Evidence captured from failed operations
- Readiness check results

## Rate limiting (HTTP 429)

The registry may ask clients to slow down. Shipper retries with exponential backoff + jitter and a max wall-clock limit.

### Retry behavior

Shipper implements a sophisticated retry strategy:

1. **Exponential backoff** - Delay increases exponentially between attempts
2. **Jitter** - Random variation in delay to avoid thundering herd
3. **Max attempts** - Configurable maximum number of retries (default: 6)
4. **Max delay** - Upper bound on backoff delay (default: 2m)

### Configurable retry options

```bash
# Adjust retry behavior
shipper publish --max-attempts 10 --base-delay 5s --max-delay 5m
```

## Permission mismatches

A common failure mode is having rights to publish some crates in a workspace but not all. Shipper can optionally preflight owners/permissions before publishing anything.

### Ownership checks

Shipper provides two levels of ownership verification:

1. **Best-effort check** (`--skip-ownership-check=false`) - Checks ownership if a token is available
2. **Strict check** (`--strict-ownership`) - Fails preflight if ownership checks fail or if no token is available

```bash
# Enable strict ownership checks
shipper preflight --strict-ownership
```

## CI cancellations

If your CI cancels a job mid-publish, you can re-run the job and Shipper will continue from the persisted state.

### Lock files (v0.2)

Shipper now uses lock files to prevent concurrent publish operations:

- `.shipper/lock` - Prevents multiple shipper instances from running simultaneously
- Configurable timeout (default: 1h) for stale lock cleanup

```bash
# Force override of existing locks (use with caution)
shipper publish --force

# Adjust lock timeout
shipper publish --lock-timeout 30m
```

## Readiness failures (v0.2)

A crate may appear to publish successfully but not be immediately available on the registry. Shipper's readiness checks verify actual registry visibility before proceeding.

### Readiness methods

Shipper supports three readiness verification methods:

1. **API** (default, fast) - Queries the registry API to check if the crate exists
2. **Index** (slower, more accurate) - Checks the crate index for the new version
3. **Both** (slowest, most reliable) - Verifies using both methods

```bash
# Use index-based readiness
shipper publish --readiness-method index

# Use both methods for maximum reliability
shipper publish --readiness-method both

# Configure timeout and poll interval
shipper publish --readiness-timeout 10m --readiness-poll 5s

# Disable readiness checks (advanced users only)
shipper publish --no-readiness
```

### Readiness timeout

If readiness checks fail, Shipper will:

1. Retry with exponential backoff
2. Wait up to the configured timeout (default: 5m)
3. Fail the publish if the crate doesn't become visible

## Evidence and debugging (v0.2)

### Event log

The event log (`.shipper/events.jsonl`) provides a complete audit trail:

```json
{"timestamp":"2024-02-10T15:30:00Z","event":"preflight_start","details":{...}}
{"timestamp":"2024-02-10T15:30:05Z","event":"preflight_complete","details":{...}}
{"timestamp":"2024-02-10T15:30:10Z","event":"publish_start","crate":"my-crate","version":"0.1.0",...}
{"timestamp":"2024-02-10T15:30:30Z","event":"publish_complete","crate":"my-crate","version":"0.1.0",...}
```

### Receipt format

The receipt (`.shipper/receipt.json`) contains:

```json
{
  "version": "0.2.0",
  "timestamp": "2024-02-10T15:30:00Z",
  "workspace_root": "/path/to/workspace",
  "published": [
    {
      "name": "my-crate",
      "version": "0.1.0",
      "published_at": "2024-02-10T15:30:30Z",
      "evidence": {
        "stdout": "...",
        "stderr": "...",
        "exit_code": 0
      }
    }
  ],
  "failed": [],
  "events_file": ".shipper/events.jsonl"
}
```

### Cleaning up

After successful publishes, you may want to clean up state files:

```bash
# Clean all state files
shipper clean

# Keep the receipt for audit purposes
shipper clean --keep-receipt
```

## Common failure scenarios

### Scenario 1: Network timeout during upload

**Symptoms**: `cargo publish` times out, but crate appears on registry

**How Shipper handles**:
1. Captures evidence from the failed command
2. Checks registry for crate existence
3. If found, marks as successful and continues
4. If not found, retries with backoff

**Debug with**:
```bash
shipper inspect-events
shipper inspect-receipt
```

### Scenario 2: Rate limiting (HTTP 429)

**Symptoms**: Registry returns 429 Too Many Requests

**How Shipper handles**:
1. Recognizes retryable error
2. Waits with exponential backoff + jitter
3. Retries up to max attempts
4. Logs all attempts in event log

**Debug with**:
```bash
shipper inspect-events | grep "retry"
```

### Scenario 3: CI cancellation mid-publish

**Symptoms**: Some crates published, job cancelled

**How Shipper handles**:
1. State file tracks progress
2. Resume skips published crates
3. Continues with remaining crates

**Debug with**:
```bash
shipper status
shipper inspect-receipt
```

### Scenario 4: Permission denied

**Symptoms**: `cargo publish` fails with permission error

**How Shipper handles**:
1. Captures detailed error evidence
2. Logs failure in event log
3. Continues with other crates if possible
4. Provides clear error message

**Prevent with**:
```bash
shipper preflight --strict-ownership
```

### Scenario 5: Registry not ready

**Symptoms**: Publish succeeds but crate not immediately available

**How Shipper handles**:
1. Performs readiness checks
2. Retries with backoff until timeout
3. Fails if crate doesn't become visible
4. Logs all readiness attempts

**Configure with**:
```bash
shipper publish --readiness-method both --readiness-timeout 10m
```

## Getting help

If you encounter a failure mode not covered here:

1. Use `shipper inspect-events` to see the complete event log
2. Use `shipper inspect-receipt` to see captured evidence
3. Use `shipper doctor` to check your environment and auth
4. Run `shipper clean` to reset state if needed
5. File an issue with the event log and receipt attached
