# Failure modes and how shipper handles them

Publishing a Rust workspace is an **irreversible, non-atomic workflow**: once a
version is uploaded to a registry it cannot be re-published. Shipper is designed
around this constraint — every step is persisted, classified, and recoverable.

---

## Error classification

When `cargo publish` fails, Shipper inspects the combined stdout/stderr output
and classifies the error into one of three classes
(see `crates/shipper-cargo-failure`):

| Class | Meaning | Action taken |
|---|---|---|
| **Retryable** | Transient — likely to succeed on retry | Retry with backoff |
| **Permanent** | Requires human intervention | Stop retrying, record failure |
| **Ambiguous** | Outcome unclear (upload may have succeeded) | Verify against registry, then retry or accept |

### Retryable patterns

Matched case-insensitively in cargo output:

`too many requests`, `429`, `timeout`, `timed out`, `connection reset`,
`connection refused`, `connection closed`, `dns`, `tls`,
`temporarily unavailable`, `failed to download`, `failed to send`,
`server error`, `500`, `502`, `503`, `504`, `broken pipe`,
`reset by peer`, `network unreachable`

### Permanent patterns

`failed to parse manifest`, `invalid`, `missing`, `license`, `description`,
`readme`, `repository`, `could not compile`, `compilation failed`,
`failed to verify`, `package is not allowed to be published`,
`publish is disabled`, `yanked`,
`forbidden`, `permission denied`, `not authorized`, `unauthorized`,
`version already exists`, `is already uploaded`, `token is invalid`,
`invalid credentials`, `checksum mismatch`

### Ambiguous fallback

If **no** pattern matches, the error is classified as **Ambiguous**. Shipper
then checks the registry to determine whether the version was actually uploaded
before deciding to retry.

---

## Retry behavior

Retries are handled by `crates/shipper-retry`. By default Shipper uses
exponential backoff with jitter (the `Default` policy).

### Default retry policy

| Parameter | Default | Description |
|---|---|---|
| `strategy` | `exponential` | `base_delay × 2^(attempt-1)` |
| `max_attempts` | **6** | Total tries (first attempt + 5 retries) |
| `base_delay` | **2 s** | Initial delay |
| `max_delay` | **120 s** (2 min) | Delay cap |
| `jitter` | **0.5** | ±50 % random variation on each delay |

### Predefined policies

| Policy | Strategy | Max attempts | Base delay | Max delay | Jitter |
|---|---|---|---|---|---|
| **default** | exponential | 6 | 2 s | 120 s | 0.5 |
| **aggressive** | exponential | 10 | 500 ms | 30 s | 0.3 |
| **conservative** | linear | 3 | 5 s | 60 s | 0.1 |

### Delay calculation example (default policy, no jitter)

| Attempt | Delay |
|---|---|
| 1 | 2 s |
| 2 | 4 s |
| 3 | 8 s |
| 4 | 16 s |
| 5 | 32 s |
| 6 | 64 s |

With jitter of 0.5, each delay is multiplied by a random factor in `[0.5, 1.5]`.

### Per-error-class overrides

In `.shipper.toml` you can set different retry parameters for each error class:

```toml
[retry]
policy = "custom"
strategy = "exponential"
max_attempts = 8
base_delay = "3s"
max_delay = "90s"
jitter = 0.4

[retry.per_error.retryable]
max_attempts = 10
base_delay = "1s"

[retry.per_error.ambiguous]
max_attempts = 4
base_delay = "5s"
```

### CLI overrides

```bash
shipper publish --max-attempts 10 --base-delay 5s --max-delay 5m
```

---

## State file format (`state.json`)

Shipper persists progress to `.shipper/state.json` after every package
completes. This file enables `shipper resume` to skip already-published crates.

```json
{
  "state_version": "shipper.state.v1",
  "plan_id": "a1b2c3d4",
  "registry": {
    "name": "crates-io",
    "api_base": "https://crates.io",
    "index_base": "https://index.crates.io"
  },
  "created_at": "2025-06-01T12:00:00Z",
  "updated_at": "2025-06-01T12:05:30Z",
  "packages": {
    "my-core@0.3.0": {
      "name": "my-core",
      "version": "0.3.0",
      "attempts": 1,
      "state": { "state": "published" },
      "last_updated_at": "2025-06-01T12:02:00Z"
    },
    "my-cli@0.3.0": {
      "name": "my-cli",
      "version": "0.3.0",
      "attempts": 0,
      "state": { "state": "pending" },
      "last_updated_at": "2025-06-01T12:00:00Z"
    }
  }
}
```

### Key fields

| Field | Purpose |
|---|---|
| `state_version` | Schema version (`shipper.state.v1`); used for forward-compatibility |
| `plan_id` | Deterministic hash of the publish plan; `shipper resume` refuses to continue if the workspace changed |
| `registry` | Target registry name, API base URL, and optional sparse-index URL |
| `packages` | `BTreeMap<"name@version", PackageProgress>` — one entry per planned crate |

### Package states

Each package in the state file has one of these states:

| State | Meaning |
|---|---|
| `pending` | Not yet attempted |
| `uploaded` | `cargo publish` exited 0 but readiness not yet confirmed |
| `published` | Confirmed visible on the registry — **terminal success** |
| `skipped` | Intentionally skipped (e.g. already on registry), includes `reason` |
| `failed` | Permanently failed, includes `class` (`retryable`/`permanent`/`ambiguous`) and `message` |
| `ambiguous` | Outcome unclear, includes `message` |

---

## Failure mode: Partial publish

**Scenario:** A workspace has crates `core`, `macros`, and `cli`. Publishing
`core` succeeds, then the network drops during `macros`.

**What happens:**
1. `core@0.3.0` is marked `published` in `state.json`.
2. `macros@0.3.0` fails — Shipper classifies as `Retryable`, retries up to
   `max_attempts`. If all retries fail, the state is saved as `failed`.
3. `cli@0.3.0` remains `pending`.

**Recovery:**
```bash
# Fix the network, then resume from where you left off:
shipper resume

# Or equivalently, re-run publish (it detects existing state):
shipper publish
```

Shipper reads `state.json`, confirms the `plan_id` matches the current
workspace, skips `core` (already `published`), and retries from `macros`.

---

## Failure mode: Ambiguous timeout

**Scenario:** `cargo publish -p macros` times out after 30 s. The upload may
or may not have reached the registry.

**What happens:**
1. No retryable/permanent pattern matches the stderr — classified as **Ambiguous**.
2. Shipper queries the registry API: does `macros@0.3.0` exist?
3. **If found:** marks `published`, continues to next crate.
4. **If not found:** retries with backoff.

**Recovery:**
```bash
# Usually no action needed — Shipper self-heals by checking the registry.
# If all retries are exhausted:
shipper resume
```

**Inspect evidence:**
```bash
shipper inspect-events     # chronological event log
shipper inspect-receipt    # structured receipt with attempt details
```

---

## Failure mode: Rate limiting (HTTP 429)

**Scenario:** crates.io returns `429 Too Many Requests` during a batch publish.

**What happens:**
1. Shipper matches `429` / `too many requests` in stderr → **Retryable**.
2. Applies exponential backoff: 2 s → 4 s → 8 s → 16 s → 32 s → 64 s (defaults).
3. Each retry is logged to `.shipper/events.jsonl`.
4. After 6 attempts (default), the package is marked `failed` with class
   `retryable` and the run continues with remaining crates.

**Recovery:**
```bash
# Wait a few minutes for the rate limit to clear, then:
shipper resume
```

**Tuning for large workspaces:**
```bash
# More retries with longer backoff
shipper publish --max-attempts 10 --base-delay 5s --max-delay 5m
```

---

## Failure mode: CI cancellation

**Scenario:** A GitHub Actions job is cancelled (timeout, manual cancel, or a
new push) after `core` and `macros` are published but before `cli`.

**What happens:**
1. The process is killed. `.shipper/state.json` reflects the last persisted
   state (state is saved after each crate completes).
2. `core` and `macros` show `published`; `cli` shows `pending`.

**Recovery:** Re-run the CI job. Shipper detects the existing state and resumes:
```bash
shipper publish   # or: shipper resume
```

### Lock file safety

Shipper writes `.shipper/lock` to prevent concurrent runs. If a CI runner is
killed without cleanup, the lock may become stale. Shipper automatically
expires stale locks (default: 1 h).

```bash
# Force-clear a stale lock
shipper publish --force

# Adjust lock timeout
shipper publish --lock-timeout 30m
```

---

## CI-specific guidance

### GitHub Actions

```yaml
name: Publish
on:
  push:
    tags: ['v*']
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install shipper-cli --locked

      # Restore state from a previous (possibly failed) run
      - uses: actions/download-artifact@v7
        with:
          name: shipper-state
          path: .shipper
        continue-on-error: true  # first run has no artifact

      - run: shipper publish --quiet
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}

      # Always save state — even on failure — so the next run can resume
      - uses: actions/upload-artifact@v6
        if: always()
        with:
          name: shipper-state
          path: .shipper
```

**Key patterns:**
- `continue-on-error: true` on the download step so the first run doesn't fail.
- `if: always()` on the upload step so state is saved even when publish fails.
- Re-running the job automatically resumes from the saved state.

### GitLab CI

```yaml
publish:
  image: rust:latest
  stage: publish
  rules:
    - if: $CI_COMMIT_TAG
  cache:
    key: ${CI_COMMIT_REF_SLUG}
    paths:
      - .shipper/
      - target/
  script:
    - cargo install shipper-cli --locked
    - shipper publish --quiet
  variables:
    CARGO_REGISTRY_TOKEN: $CARGO_REGISTRY_TOKEN
  artifacts:
    paths:
      - .shipper/
    expire_in: 1 day
    when: always  # save state on failure too
```

**Key patterns:**
- `cache` persists `.shipper/` across pipeline retries on the same branch/tag.
- `when: always` on artifacts ensures state survives failed jobs.
- Click **Retry** in the GitLab UI and Shipper picks up where it left off.

---

## Evidence and debugging

### Event log (`.shipper/events.jsonl`)

Append-only, one JSON object per line:

```json
{"timestamp":"2025-06-01T12:00:00Z","event_type":{"type":"execution_started"},"package":""}
{"timestamp":"2025-06-01T12:01:00Z","event_type":{"type":"package_started","name":"my-core","version":"0.3.0"},"package":"my-core@0.3.0"}
{"timestamp":"2025-06-01T12:01:05Z","event_type":{"type":"package_attempted","attempt":1,"command":"cargo publish -p my-core"},"package":"my-core@0.3.0"}
{"timestamp":"2025-06-01T12:01:20Z","event_type":{"type":"package_published","duration_ms":15000},"package":"my-core@0.3.0"}
```

### Receipt (`.shipper/receipt.json`)

Written after the run completes. Contains per-package evidence (every attempt's
command, exit code, stdout/stderr tail, and timing) plus git context and
environment fingerprint. See [the types in `crates/shipper-types`](../crates/shipper-types/src/lib.rs)
for the full schema.

### Inspection commands

```bash
shipper inspect-events                 # human-readable event timeline
shipper inspect-receipt                # formatted receipt summary
shipper inspect-receipt --format json  # machine-readable for scripts
shipper status                         # compare local versions vs registry
shipper doctor                         # check environment, auth, tools
```

### Cleaning up

```bash
shipper clean                 # remove all state files
shipper clean --keep-receipt  # keep receipt.json for auditing
```

---

## Getting help

If you encounter a failure not covered above:

1. Run `shipper doctor` to check your environment.
2. Inspect `.shipper/events.jsonl` and `.shipper/receipt.json` for evidence.
3. File an issue with the event log and receipt attached.
