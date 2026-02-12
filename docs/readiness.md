# Readiness Checking

Readiness checking ensures that published crates are actually visible on the registry before proceeding with subsequent publishes. This is critical for workspace publishing where later packages depend on earlier ones.

## What Readiness Checking Does

After successfully publishing a crate, Shipper verifies that the crate is available on the registry before continuing. This prevents failures where a later package tries to depend on a version that hasn't propagated yet.

Readiness checking is especially important for:

1. **Workspace publishing** - Where packages depend on each other
2. **CI/CD pipelines** - Where timing is critical
3. **Large workspaces** - Where propagation delays can cause issues

## Readiness Methods

Shipper supports three readiness verification methods:

| Method | Speed | Accuracy | Use Case |
|--------|-------|----------|----------|
| **API** | Fast | Good | Default choice for most users |
| **Index** | Slower | High | When API is unreliable or slow |
| **Both** | Slowest | Highest | Critical production publishes |

### API Method (Default)

Queries the registry's HTTP API to check if the crate version exists.

**Advantages:**
- Fast - typically completes in seconds
- Low overhead
- Works well for most registries

**Disadvantages:**
- May have propagation delays
- API rate limits may apply

```bash
# Use API-based readiness (default)
shipper publish --readiness-method api
```

### Index Method

Checks the sparse index for the crate version entry.

**Advantages:**
- More accurate - directly verifies the crate index
- Less affected by API rate limits
- Better for large registries

**Disadvantages:**
- Slower - requires downloading and parsing index files
- More network overhead

```bash
# Use index-based readiness
shipper publish --readiness-method index
```

### Both Method

Verifies using both API and index methods for maximum reliability.

**Advantages:**
- Highest reliability - confirms through multiple sources
- Reduces false positives/negatives

**Disadvantages:**
- Slowest - performs both checks
- Highest network overhead

```bash
# Use both methods for maximum reliability
shipper publish --readiness-method both

# Prefer index over API when using both
shipper publish --readiness-method both --prefer-index
```

## Configuring Readiness Checking

### Configuration File

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

### CLI Flags

```bash
# Enable/disable readiness checks
shipper publish --no-readiness

# Set readiness method
shipper publish --readiness-method api
shipper publish --readiness-method index
shipper publish --readiness-method both

# Configure timeout
shipper publish --readiness-timeout 10m

# Configure poll interval
shipper publish --readiness-poll 5s

# Prefer index when using both
shipper publish --readiness-method both --prefer-index
```

### Configuration Options

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

## How Readiness Checking Works

1. **Initial Delay**: Wait for `initial_delay` before first check
2. **Polling**: Check visibility at `poll_interval` with exponential backoff
3. **Jitter**: Apply random variation to delays to avoid thundering herd
4. **Timeout**: Fail if not visible within `max_total_wait`

### Polling Behavior

Shipper uses exponential backoff with jitter for polling:

```
Attempt 1: delay = initial_delay
Attempt 2: delay = min(poll_interval * 2, max_delay)
Attempt 3: delay = min(poll_interval * 4, max_delay)
...
```

Jitter is applied to each delay:

```
actual_delay = delay * (1 ± jitter_factor)
```

## Troubleshooting Readiness Issues

### Issue: Readiness timeout exceeded

**Symptoms**: Publish fails with "readiness timeout" error

**Cause**: The crate didn't become visible within the configured timeout

**Solutions**:
1. Increase the timeout: `--readiness-timeout 10m`
2. Check if the registry is experiencing issues
3. Verify the crate was actually uploaded
4. Use `shipper inspect-events` to see the readiness check attempts

### Issue: Readiness checks are slow

**Symptoms**: Readiness checks take a long time

**Cause**: Index-based checks can be slow for large registries

**Solutions**:
1. Use API-based readiness: `--readiness-method api`
2. Increase the poll interval to reduce check frequency: `--readiness-poll 5s`
3. Use a local index mirror

### Issue: Readiness checks fail even though crate was published

**Symptoms**: Publish succeeded but readiness checks fail

**Cause**: Registry propagation delay or API/index inconsistency

**Solutions**:
1. Use both methods: `--readiness-method both`
2. Increase the timeout to allow propagation
3. Check the registry manually to confirm the crate exists
4. Use `shipper inspect-events` to see detailed readiness attempts

### Issue: Index checks are slow

**Symptoms**: Readiness checks take a long time when using `index` or `both` methods

**Cause**: The sparse index is large and checking it requires downloading and parsing index files

**Solutions**:
1. Use API-based readiness for faster checks: `--readiness-method api`
2. Increase the timeout: `--readiness-timeout 10m`
3. Use a local index mirror for faster access

### Issue: Index shows stale data

**Symptoms**: Index checks fail even though the crate was successfully published

**Cause**: The sparse index hasn't been updated yet (propagation delay)

**Solutions**:
1. Use API-based readiness instead: `--readiness-method api`
2. Use both methods: `--readiness-method both --prefer-index=false`
3. Increase the timeout to allow index propagation
4. Manually update the index: `cargo update`

### Issue: Custom index path not found

**Symptoms**: Readiness checks fail with "index path not found" error

**Cause**: The configured `index_path` doesn't exist or is not accessible

**Solutions**:
1. Verify the index path is correct
2. Remove the `index_path` setting to use the default index
3. Ensure the path is accessible to the shipper process

## Performance Considerations

### Choosing the Right Method

- **For fast publishes**: Use `api` method
- **For reliability**: Use `both` method
- **For large registries**: Use `index` method if API is slow
- **For CI/CD**: Use `both` method with increased timeout

### Optimizing for Speed

```toml
[readiness]
# Faster but less reliable configuration
enabled = true
method = "api"
initial_delay = "500ms"
poll_interval = "1s"
max_total_wait = "2m"
```

### Optimizing for Reliability

```toml
[readiness]
# Slower but more reliable configuration
enabled = true
method = "both"
prefer_index = true
initial_delay = "2s"
poll_interval = "3s"
max_total_wait = "10m"
```

### Reducing Registry Load

```toml
[readiness]
# Reduce poll frequency to minimize registry load
enabled = true
method = "api"
initial_delay = "5s"
poll_interval = "10s"
max_total_wait = "5m"
jitter_factor = 0.8  # High jitter to spread out requests
```

## Disabling Readiness Checks

**Warning**: Disabling readiness checks can lead to publish failures in workspace scenarios where packages depend on each other. Only disable if you understand the risks.

```bash
# Disable readiness checks (not recommended for workspaces)
shipper publish --no-readiness
```

Or in configuration:

```toml
[readiness]
enabled = false
```

## Best Practices

1. **Always enable readiness checks for workspaces** - Prevents dependency failures
2. **Use API method for speed in most cases** - Fast and reliable enough for most users
3. **Use Both method for critical publishes** - Maximum reliability for production
4. **Adjust timeout based on registry performance** - Some registries are slower than others
5. **Monitor readiness check times** - Use `shipper inspect-events` to track performance
6. **Use jitter to avoid thundering herd** - Especially important for CI/CD with multiple jobs

## Related Documentation

- [Configuration](configuration.md) - Configuration file options
- [Failure Modes](failure-modes.md) - Common failure scenarios and solutions
- [Preflight](preflight.md) - Pre-flight verification
- [README](../README.md) - Main documentation
