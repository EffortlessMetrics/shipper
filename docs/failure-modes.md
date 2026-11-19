# Failure modes and how shipper handles them

Publishing a Rust workspace is an **irreversible, non-atomic workflow**: once a
version is uploaded to a registry it cannot be re-published. Shipper is designed
around this constraint — every step is persisted, classified, and recoverable.

---

## Error classification

An invocation whose process cleanup cannot be confirmed stopped has a separate
`CleanupIncomplete` safety hold. This takes precedence over Cargo's exit code,
timeout, diagnostic classification and registry reconciliation. The local hold
must not authorize another upload even when the registry reports absence.

When the Cargo process exceeds Shipper's execution timeout, the shared package
executor classifies it as **Ambiguous** using the typed timeout result. This
decision is independent of output text and the configured tail length. An
unsuccessful publish whose diagnostic capture was incomplete also enters
Ambiguous reconciliation. For other unsuccessful Cargo exits with complete
capture, Shipper inspects the combined stdout/stderr
and uses the patterns in `crates/shipper-cargo-failure`:

| Class | Meaning | Action taken |
|---|---|---|
| **Retryable** | Transient — likely to succeed on retry | Retry with backoff |
| **Permanent** | Requires human intervention | Stop retrying, record failure |
| **Ambiguous** | Outcome unclear (upload may have succeeded) | Reconcile against registry truth; accept `Published`, retry only a proved `NotPublished`, or stop on `StillUnknown` |

### Bounded publish diagnostics

Publication and rehearsal retain the configured last-N-lines diagnostic tail
within fixed limits: 256 KiB per stream, 64 KiB per logical line, 4096 retained
lines, and an 8 KiB read buffer. A very large `--output-lines` value does not
remove those bounds. Output continues to drain after a limit is reached.

A capacity limit or read failure makes capture incomplete for that invocation.
Both tails are then omitted; stderr contains
`[shipper: Cargo diagnostics omitted after capture loss]`. This conservative
policy prevents a secret suffix or continuation from surviving after its
redaction context was discarded. `--output-lines 0` still returns empty tails.
The process exit code and timeout flag are preserved. After complete process
cleanup, successful Cargo proceeds through upload/readiness as usual;
unsuccessful incomplete capture must
reconcile before another upload. The omission message is explanatory; the engine
uses typed completeness to make that decision.

These limits apply to publication capture, including the public Cargo publish
wrapper. Metadata and generic complete-output runners are unchanged, as are
yank, install and dry-run retention.

### Unix subprocess cleanup

On Linux and macOS, publication (with or without an execution timeout) and generic
commands with an execution timeout use nonblocking stdout/stderr reads. The
command starts in its own process group. Once the direct child exits or the
execution deadline expires, Shipper sends a termination signal to that group and
allows up to one additional second to drain the pipes, reap the direct child,
and observe that the group is gone. OS scheduling and the spawn operation are
not hard real-time guarantees.

A failed group signal is provisional. If the direct child is then reaped, the
group is proved absent, and both pipes reach EOF, cleanup is complete. If those
facts remain unproved at the deadline, the invocation retains the cleanup hold.

On Linux, orphaned zombie descendants can keep the group probe positive even
after all active work has stopped. This can occur in containers whose PID 1 does
not reap orphaned children. Shipper conservatively retains the hold; it does not
treat a zombie-only group as absent. Use a container init that reaps children to
avoid accumulating this condition.

An early direct-child exit keeps its exit code and `timed_out=false`, including
when its descendants inherited output handles. Only exceeding the execution
deadline sets `timed_out=true`. Known incomplete cleanup is a separate typed
result; APIs that expose only the legacy output fields return an error instead
of silently dropping that result. Closing an unread pipe also makes diagnostic
capture incomplete, preserving the fixed omission policy above.

The termination signal uses the group identity while its original leader is
still unreaped. Later checks are read-only, so PID reuse cannot redirect a kill
to another invocation. Unrelated processes outside the group are unaffected.
This contains cooperative descendants. A descendant that deliberately changes
its group or session is outside that boundary: an escaped descendant retaining
a pipe is detected as incomplete cleanup, while one that also closes both pipes
cannot be discovered through this mechanism.

### Windows subprocess cleanup

On Windows, publication (with or without an execution timeout) and generic
commands with an execution timeout use a fresh kill-on-close Job Object and
nonblocking stdout/stderr capture. Shipper assigns the direct child to the job
before it starts running. On direct-child exit, timeout, or interruption,
Shipper requests termination of the owned job and allows up to one additional
second to confirm direct-child exit, an empty job, and EOF on both pipes. The
direct child's observed exit code is retained; cleanup does not itself set
`timed_out=true`. If cleanup cannot be confirmed, Shipper retains the
[cleanup hold](#retained-process-cleanup-hold) instead of retrying. A process
outside the job is not terminated; if it still holds an inherited pipe open,
missing EOF also causes a hold. The allowance is not a hard real-time guarantee
for process creation or OS scheduling.

Platforms other than Linux, macOS, and Windows still use the existing
direct-child/reader fallback. The bounded cleanup described here does not
apply to that fallback.

### Termination during publication

The shipped `shipper` and `shipper-cli` executables install process-lifetime
handling for SIGINT, SIGTERM and SIGHUP on Linux and macOS, and Ctrl+C and
Ctrl+Break console events on Windows. During supervision, an interruption
requests bounded cleanup of the owned Cargo group or job and stops later
commands. For live publish/resume, confirmed cleanup retains the attempt with an
ambiguous upload outcome. The command returns an interruption error without an
in-run retry. A fresh `resume` reconciles that attempt against the registry
before any replacement upload. An interruption does not establish that the
registry rejected the upload.
If cleanup cannot be confirmed, the absorbing cleanup hold below takes precedence.
Rehearsal instead retains its existing `rehearsal_package_failed` event with an
ambiguous class; it does not create a resumable live-publish attempt projection.

Deferred termination remains owned through the interrupted or uncertain result's
persistence attempt, including parallel peers. Outside those intervals, signals
retain ordinary termination behavior; planning, untimed metadata/dry-run work,
registry waits and backoff do not acquire a new cancellation-latency guarantee.
The shutdown request is sticky within the process and is never reset between
packages or retries. SIGKILL, process crashes, arbitrary external reapers and
processes outside the owned group or job remain outside this cooperative path.

Plain `shipper_cli::run()` and core embedding entry points install no handlers.
The executable-only `run_process()` wrapper owns this policy; embedders retain
responsibility for their host's process and signal lifecycle. Focused native
evidence covers Linux signals and Windows cleanup and OS-delivered Ctrl+Break
with fixture Cargo and a loopback registry. It does not establish native macOS
behavior, all Windows console-event paths, or final release-binary support.

### Retained process cleanup hold

Known incomplete cleanup records `package_cleanup_incomplete` in `events.jsonl`
and projects the affected package as `cleanup_incomplete` in `state.json`. Its
attempt is retained with an unknown upload outcome and no scheduled retry. This
is a local process condition: registry visibility or absence cannot clear it,
and it does not change ordinary `Permanent` failure recovery.

The hold applies to the entire state directory, including a package omitted by
the current plan or `--resume-from`. Publish, resume, direct parallel dispatch
and repeated rehearsal check both retained artifacts before starting new work.
`--force`, `--force-resume` and a higher attempt ceiling do not override it.
Rehearsal records the same hold event without inventing a normal publish state.
The CLI reports `safe_to_resume=false` with investigation guidance; it offers no
automatic hold-clearing command. `shipper clean` preserves a readable hold even
when the other artifact is corrupt or another event line is malformed. A
complete, typed hold record before or after the malformed line is sufficient
to refuse cleanup; Shipper neither repairs the history nor prints the malformed
payload. If the hold record itself is unreadable or no longer recognizable,
publish/resume still refuse corrupt evidence, while explicit clean keeps its
existing corrupt-artifact recovery behavior.

Within parallel execution, attempt admission and hold publication share the
state lock. A peer whose attempt was already committed may finish, while every
later attempt or retry is refused. All admitted peers are joined before the
scheduler returns. A worker error or synchronization error cannot turn a known
typed cleanup stop into ordinary retry advice.

The observed hold enters memory before its event/state transition. If appending
the event fails, Shipper attempts to retain the hold through the existing state
writer. An offline or exclusively invoked rebuild cannot erase an already
retained projection-only hold when its authoritative event is missing; it
refuses and preserves the artifacts. The low-level public rebuild writer does
not acquire ownership and must not run concurrently with publication, cleanup
or other embedding writes. A hold
event also blocks when state is missing, stale or corrupt, and ordinary later
events cannot clear a hold during rebuild. If both persistence writes fail, the
current invocation still stops, but no durable refusal is promised for a later
invocation; the error explicitly says that no safe rerun is established.
Rehearsal has only the hold-event persistence path; an append failure has the
same limit on refusal in a later invocation.

This does not add a crash-proof process fence or a general process supervisor.
A crash before recording the hold, deliberate escape with closed pipes, manual
evidence removal, unsynchronized embedding writes and older binaries are
outside this guarantee. Preserve the
artifacts and use a binary that understands the hold vocabulary when inspecting
such a run. In particular, some older publish entry points ignore historical
events or trust caller-supplied state, so downgrading is not a supported way to
recover a cleanup hold.

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
then checks the registry to determine whether the version was actually
uploaded. It never converts an inconclusive `StillUnknown` result into a blind
retry.

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

### Live retry progress

Sequential and parallel publishing deliver retry progress during the wait before
the next Cargo attempt. The displayed delay is the time remaining on the package
worker's monotonic backoff deadline. Rendering a countdown does not add another
retry wait or determine whether an attempt is allowed.

Information, warnings, errors, and retry notices retain their enqueue order, so a
failed-attempt warning precedes its retry notice and later success message. Across
parallel packages, this is the order in which workers enqueue their messages.
Slow reporter callbacks can delay narration while workers continue with the same
retry policy and durable outcomes. Callbacks run on the caller's thread and do not
need to be `Send`. Human progress stays on stderr; quiet mode suppresses it and
JSON publish output remains one command envelope.

### Global retry configuration

Configure one retry policy in `.shipper.toml`:

```toml
[retry]
policy = "custom"
strategy = "exponential"
max_attempts = 8
base_delay = "3s"
max_delay = "90s"
jitter = 0.4
```

Per-error retry policies are **unsupported**. Earlier versions parsed
`[retry.per_error.retryable]`, `[retry.per_error.ambiguous]`, and
`[retry.per_error.permanent]` without applying them. Remove those class entries
and use the global `[retry]` policy above. Config validation and public engine
entry points now reject any configured class, including an empty class subtable,
before state/event writes, registry requests, notifications, or Cargo publish,
dry-run, or yank invocation. CLI workspace discovery may run Cargo metadata first.
An omitted `per_error` or an empty `[retry.per_error]` table keeps global behavior.
Unknown keys in `per_error` are rejected during decoding.

The package-wide `max_attempts` ceiling remains cumulative across publish and
resume. Permanent failures stop automatic retries. Ambiguous outcomes reconcile
against the registry and retry only after a proved `NotPublished`; an inconclusive
`StillUnknown` stops. This refusal does not change those rules or delay floors.

For embedders inspecting old configurations, the historical
`RetryStrategyConfig` representation remains parseable: an omitted serialized
`max_attempts` is `0`, while `RetryStrategyConfig::default()` uses `6`. Diagnostic
loading preserves that difference; it does not authorize execution of a class
policy. Use the validated global policy for execution.

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
| `plan_id` | Deterministic hash of the registry API base and ordered package names/versions; resume refuses a stored/current mismatch, but this is not source or workspace provenance |
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

**Recovery:** inspect the typed local posture before choosing a command:
```bash
shipper status --durable
shipper inspect-events
shipper inspect-receipt --format json
```

Resume only when the durable outcome reports a coherent interrupted run and its
typed next action recommends `resume`. Shipper then confirms the current
`plan_id`, skips terminal packages, and continues from eligible pending or
retryable work. A different checkout can produce the same plan ID, so verify
the restored source independently.

Before another Cargo attempt, Shipper checks registry visibility for pending
packages and retained `failed` packages classified as `retryable`. A version
already visible is recorded as `skipped` with reason `already published`, while
its cumulative attempt count and prior event history are retained. This applies
to sequential and parallel execution. A failed precheck is reported as a warning
and keeps the existing retry policy for this non-ambiguous work; it is not
recorded as proof of absence. Ambiguous and uploaded packages continue through
their dedicated reconciliation paths. This check does not reset retry ceilings
or change permanent-error classification.

### Controlled-stop admission

`publish` and `resume` apply the same retained-evidence check before continuing
a controlled `NotPublished` stop. The current terminal stop marker, state
projection and reconciliation identity must agree, no completed receipt may be
present, and the predecessor must be absent or conclusively not live. Live or
unknown predecessor evidence refuses continuation even with `--force`. The
selected packages must have room under the cumulative `--max-attempts` ceiling;
raising that ceiling deliberately does not reset prior attempts or history.
Current-attempt events also identify when a controlled-stop check is required:
a `NotPublished` reconciliation followed by a retryable failure cannot evade
validation merely because both the terminal marker and derived report identity
are damaged. This event signal requires the complete evidence check; it never
authorizes continuation by itself. Superseding attempts and outcomes retain
the existing ordinary-failure, ambiguous-upload and uploaded-package recovery.

An early check provides diagnostics, then Shipper reserves local OS ownership
and rereads the durable evidence before claiming holder metadata, recording a
new execution start, notifying webhooks or contacting the registry. Refused
admission preserves the existing state, events, receipt, reconciliation and
holder-record bytes. `resume` always requires retained state. A `publish` caller
that already observed state also refuses if it disappears before ownership;
remaining execution evidence cannot silently initialize a replacement run.
Fresh publication, preflight/rehearsal-only history and completed idempotent
reruns remain supported. This relies on the cooperating local ownership scope
described below, not protection against external editors or legacy publishers.

The direct `shipper_core::engine::parallel::run_publish_parallel` library entry
point owns its invocation lock and applies the same admission with dependency
level scope. It reloads durable state instead of trusting a stale caller copy.
When no durable execution exists, the caller may supply an unattempted initial
projection for the current plan and registry. A separately held lock, even from
the same process, is refused because that signature does not transfer ownership.
Each admitted direct call begins a new execution segment and records coherent
controlled-stop evidence on a recoverable failure, allowing a raised-ceiling
continuation and later idempotent calls. It returns package receipts on success
without claiming `ExecutionFinished` or writing a full receipt. Call it without
a separate lock, or use `run_publish` / `run_resume` for finalization and the
completion notification. The orchestrator's private parallel adapter is used
only after the orchestrator has acquired ownership and admitted evidence.

---

## Failure mode: Ambiguous upload result

**Scenario:** `cargo publish -p macros` exits unsuccessfully after an upload,
but its output does not establish whether the registry accepted the crate.

**What happens:**
1. Cargo exceeds Shipper's process timeout, or no retryable/permanent pattern
   matches an ordinary unsuccessful exit — classified as **Ambiguous**.
2. Shipper queries the configured readiness authorities: does `macros@0.3.0` exist?
3. **If `Published`:** marks `published`, continues to the next crate.
4. **If `NotPublished`:** the proved-absent upload may retry under policy.
5. **If `StillUnknown`:** persists reconciliation evidence and stops before
   another Cargo attempt.

With readiness enabled, temporary probe errors may recover while polling. If
the final poll still cannot query a selected authority, the outcome remains
`StillUnknown`; a timeout is not proof that the version is absent. `Both`
requires conclusive API and index absence in the same final poll, while either
positive observation establishes publication. Invalid index content and
unreadable local/cache files are unknown, and failed probes are not recorded
as successful negative observations. The same rule applies on resume and in
both scheduler modes.

**Recovery:**
```bash
shipper status --durable
shipper inspect-events
```

Do not resume while registry truth is inconclusive. Follow the typed
`reconcile` posture and retain `reconciliation.json` for diagnosis.

**Inspect evidence:**
```bash
shipper inspect-events     # event log in authoritative append order
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

**Recovery:** after the rate limit clears, classify the retained run first:
```bash
shipper status --durable
```

Run `shipper resume` only when the returned next action recommends it; a
retryable label or elapsed delay alone is not authorization.

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

**Recovery:** restore the complete `.shipper/` artifact into the exact intended
checkout, use the matching candidate binary, then classify the run before
dispatch:
```bash
shipper status --durable
```

Only a coherent `interrupted` outcome can recommend `shipper resume`. Missing,
legacy, cross-host, corrupt, or mismatched identity evidence remains fail
closed; cancellation by itself does not prove that the old process is gone.

### Lock file safety

Shipper holds a persistent OS-lock sidecar while publish, resume, or cleanup
owns a workspace/state-directory key. The separate JSON record carries advisory
PID, hostname, timestamp, plan, and optional process-identity evidence. Never
remove the guard sidecar to break a lock: a replacement inode would allow two
independent owners.

```bash
# Inspect correlated local evidence before choosing a recovery action
shipper status --durable

# Set the minimum age for positively abandoned legacy-record recovery
shipper publish --lock-timeout 30m
```

An actually held OS guard always refuses another invocation, including
`--force` and `shipper clean --force`. Once the guard is available, a same-host
record from the current ownership protocol can be recovered without inferring
liveness from its age. Legacy automatic recovery requires a positive local
process-absence observation in the recorded scope and age strictly greater than
the timeout. Future timestamps, corrupt metadata, missing identity, unavailable
probes, and foreign or mismatched process scopes remain inconclusive.

Explicit `--force` can replace otherwise unknown metadata after acquiring the
guard, under the operator's assumption that any noncooperating legacy or foreign
publisher is quiescent. It cannot override a positively observed live legacy
holder, reset retry counts, or authorize another upload. Retained-state and
registry-reconciliation decisions remain separate admission requirements.
Cleanup retains ownership across all deletions and preserves artifacts when
ownership is refused.

The ownership guarantee is for cooperating processes using the same key and a
filesystem supporting the OS primitive. It does not cover older binaries that
ignore the sidecar, external deletion of the sidecar, distributed/network
filesystem semantics, path aliases, or power-loss durability. Linux process
identity supports conservative legacy recovery; other platforms retain unknown
liveness when they cannot provide that evidence.

---

## CI-specific guidance

Use [Run a release in GitHub Actions](how-to/run-in-github-actions.md) for the
maintained CI sequence, artifact boundaries, typed exit handling, and
release-authority fence. Do not copy an old “retry the job and it resumes”
snippet: CI must retain the complete evidence set and classify it from the
matching workspace before choosing a recovery command.

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

Written only after the run finalizes. Contains per-package evidence (every attempt's
command, exit code, stdout/stderr tail, and timing) plus git context and
environment fingerprint. See [the types in `crates/shipper-types`](../crates/shipper-types/src/lib.rs)
for the full schema.

### Inspection commands

```bash
shipper inspect-events                 # human-readable event timeline
shipper inspect-receipt                # formatted receipt summary
shipper inspect-receipt --format json  # machine-readable for scripts
shipper status                         # compare local versions vs registry
shipper status --watch                 # watch local persisted progress
shipper status --durable               # correlate retained recovery evidence
shipper doctor                         # check environment, auth, tools
```

`inspect-events` is authoritative history, not process-liveness proof.
`inspect-receipt` fails when no finalized receipt exists. Durable status is the
read-only local classifier; malformed evidence exits `1` before it can render a
durable result.

### Cleaning up

```bash
shipper clean                 # remove all state files
shipper clean --keep-receipt  # keep receipt.json for auditing
```

---

## Getting help

If you encounter a failure not covered above:

1. Run `shipper doctor` to check your environment.
2. Inspect `.shipper/events.jsonl`, durable status, and any finalized receipt.
3. Redact workspace/package-sensitive context as required, then file an issue
   with the relevant evidence paths and typed outcome. Never attach registry
   tokens, passphrases, credentials, or unreviewed raw environment data.
