# State files reference — `.shipper/`

One-page cheat sheet. For the full contract see [INVARIANTS.md](../INVARIANTS.md); for triage recipes see [inspect-a-stalled-run.md](../how-to/inspect-a-stalled-run.md).

## Authority order

**`events.jsonl` > `state.json` > `receipt.json`**

When they disagree, events win. `state.json` and `receipt.json` are projections/summaries derived from events. An end-of-run consistency check emits `StateEventDriftDetected` if drift is found.

That authority order applies to execution state. Other `.shipper/` artifacts
answer narrower questions: auth posture, reconciliation evidence, remediation
planning, or captured plan/preflight output.

Ordinary publish/resume requires event-file sync before projecting progress and
temporary JSON file sync before atomic replacement. A returned sync error can
leave appended event bytes or an uncommitted `.tmp` file; it does not prove that
an upload was absent. Preserve that evidence for diagnosis. After an attempted
append fails, the same in-memory `EventLog` owner refuses further writes and
retains its pending batch even after `clear()`; this hold does not survive a new
owner or invocation and does not cancel an already-running peer. Parent-directory
sync remains best effort, and neither host/power-loss durability nor the separate
encrypted helper path is covered by this replacement claim. See the precise
[persistence boundary](../INVARIANTS.md#persistence-boundary-and-failure-model).

## Per-file summary

| File | Authority | Purpose | When written | Format |
|---|---|---|---|---|
| `events.jsonl` | **Truth** (append-only) | Every state transition with timestamp | Per event | JSONL (one event per line) |
| `state.json` | Projection | Serialized `ExecutionState` for fast resume | Per package state change | JSON |
| `receipt.json` | Summary | End-of-run audit artifact with evidence | Once, at run completion | JSON |
| `lock` | — | Concurrent-publish guard and optional process-identity evidence | Held during the run | JSON-compatible `LockInfo` metadata plus optional process identity on supported Linux hosts; legacy text remains readable but cannot prove liveness |

Additional evidence artifacts may appear when the related command or workflow
runs:

| File | Authority | Purpose | When written | Format |
|---|---|---|---|---|
| `auth-evidence.json` | Auth evidence | Observed Trusted Publishing/fallback context without token values | Release workflow auth setup | JSON |
| `reconciliation.json` | Ambiguity evidence | Registry-truth evidence for ambiguous publish outcomes | When ambiguous cargo output is reconciled | JSON |
| `remediation-plan.json` | Remediation plan | Dry-run containment and fix-forward plan derived from a receipt | `shipper remediate --dry-run` | JSON |
| `plan.txt` | Captured output | Plan JSON captured for workflow artifacts | Release workflow plan stage | Text containing JSON |
| `preflight_workspace_verify.txt` | Captured output | ANSI-stripped Cargo workspace dry-run output | Preflight workspace verification | Text |

## Doctor state-path diagnostics

`shipper doctor` checks the resolved state path before calling it usable. A
regular file, another non-directory type, or a non-directory parent produces a
blocking finding. An absent path is prospective: doctor leaves it absent and
reports an unknown check with a reason. A metadata inspection error also remains unknown;
it is not converted into a missing path or a passed check.

For an existing directory, doctor exclusively creates a uniquely named
`.shipper-doctor-probe-*` file, writes a small disposable payload, and removes
that exact file. A name collision is never opened, overwritten, or removed.
Existing state, events, receipts, locks, and configuration files are not modified.
The probe does change directory metadata. A returned probe failure is blocking;
if removal fails, the finding names the probe that may remain for inspection.
Permission mode bits alone are not proof of effective access for the current user.

Text and `shipper.doctor.v1` JSON use the same finding status when counting this
check. JSON adds a typed `state_dir.observation`, including the failed operation
and I/O error kind when available. Known `exists` values remain booleans; the
field is omitted when inspection cannot establish existence. `writable: true`
requires a successful create/write/remove probe. A returned create or write
failure reports `writable: false`; untested access or unsuccessful probe removal
omits the field. Unknown findings have `status: unknown`, an explanatory reason,
and warning severity; they count as unknown checks rather than passed checks or
warnings. Definite blockers retain `status: blocked`.

This is a current access observation, not a promise of future access, evidence
consistency, invocation ownership, or host/power-loss durability. A passing
doctor check does not authorize publication.

## Compatibility note

0.5 artifacts are rebuildable for every field promised by the event vocabulary.
Shipper 0.5 can read 0.4 events, state, and receipts and safely resume those runs.
Fields introduced after 0.4 are unknown when the old artifact cannot provide
evidence unless their schema defines an explicit compatibility default. The
current exception is `Receipt.execution_result`: a missing value in a legacy
all-published receipt deserializes as `success`, as preserved by
`receipt_without_execution_result_defaults_to_success`. Consumers must not
invent defaults for other absent evidence. See
[INVARIANTS.md](../INVARIANTS.md) for the full compatibility and event-first
contract.

Use Shipper 0.5 to resume a run once `package_upload_unconfirmed`
or `package_not_launched` appears in its journal. The 0.4 reader predates both
event types.

`PackageState::CleanupIncomplete` (`cleanup_incomplete`) and
`EventType::PackageCleanupIncomplete` (`package_cleanup_incomplete`) add a local
process-safety hold to the 0.5 vocabulary. Exhaustive Rust consumers and JSON
consumers must handle these variants. Existing states and events keep their
wire shape, and the new CLI count/outcome fields are omitted when absent.
An older binary may reject the new state, but older publish paths can ignore
an event-only hold or supplied-state mismatch. Downgrade recovery of held runs
is therefore unsupported; there is no universal old-consumer fail-closed claim.

## Which file for which question?

| Question | File |
|---|---|
| What happened, in order? | `events.jsonl` |
| What's the current state (fast lookup)? | `state.json` |
| Did the whole release succeed, and what's the audit trail? | `receipt.json` |
| What would `shipper resume` skip? | `state.json` (packages with `state.state == "published"`) |
| What's the truth when they disagree? | `events.jsonl` |
| Which auth path was observed? | `auth-evidence.json` |
| How did Shipper resolve ambiguity? | `events.jsonl`; `reconciliation.json` if present |
| What would remediation do? | `remediation-plan.json` |
| Is a retained run terminal, live, interrupted, or unsafe to classify? | `shipper status --durable` over the correlated local evidence set; no single file proves this |

## Key field paths

### `events.jsonl` (one JSON object per line)

Line order is the authoritative append/replay order, including when timestamps
decrease. Events retain the original wall-clock observation time; delayed
workers and clock rollback can make timestamp order differ from the package's
causal sequence. `shipper inspect-events` and `--follow` preserve append order
within each log. See the [ordering contract](../INVARIANTS.md#event-order-and-wall-clock-time).

```json
{
  "timestamp": "2026-04-17T...",
  "event_type": {"type": "package_published", "duration_ms": 3400},
  "package": "shipper-types@0.3.0-rc.1"
}
```

Common event types:
- `plan_created` — beginning
- `preflight_started`, `preflight_workspace_verify`, `preflight_complete`
- `package_started`, `package_attempted`, `package_uploaded`, `package_published`, `package_failed`, `package_skipped`
- `package_upload_unconfirmed` — Cargo ran, and upload acceptance is still
  unknown. The event records the attempt's failure class and message.
- `package_not_launched` — the supervisor proved Cargo did not start for this
  attempt. The attempt still counts toward the retry limit.
- `package_cleanup_incomplete` — local process cleanup was not confirmed; blocks new publication regardless of registry truth
- `retry_backoff_started` — added in [#91](https://github.com/EffortlessMetrics/shipper/issues/91); carries attempt N/M, delay, reason, next-attempt time
- `publish_reconciling`, `publish_reconciled` — added in [#99](https://github.com/EffortlessMetrics/shipper/issues/99); registry-truth resolution of ambiguous outcomes
- `state_event_drift_detected` — added in [#93](https://github.com/EffortlessMetrics/shipper/issues/93); end-of-run consistency check
- `execution_started`, `execution_finished`, `execution_stopped`

`execution_stopped` is a nonterminal, no-receipt marker written while the
publish lock is still held. Its current reason,
`not_published_retry_budget_exhausted`, records that registry truth proved the
selected package absent after Cargo's ambiguous exit and the retry budget was
exhausted. The marker authorizes nothing by itself: status and resume also
require matching plan, registry, state, ordered events, and reconciliation
evidence. An unfinished run without this exact coherent marker keeps missing,
legacy, cross-host, and otherwise inconclusive lock evidence fail-closed.
For a coherent controlled stop, an absent lock or an affirmatively `NotLive`
holder permits recovery. A matching `Live` holder still blocks recovery, and
an `Unknown` observation (including corrupt, legacy, or cross-host lock
evidence) remains fail-closed. The public raw-event API therefore includes the
`ExecutionStopped` event variant and its `ControlledStopReason`; exhaustive
consumers must handle that compatibility surface.

`package_cleanup_incomplete` carries a `message` and a package key. It is an
absorbing local hold for this state directory, independent of plan membership,
registry reconciliation, attempt ceiling, or lock-holder liveness. It can also
originate from rehearsal, which has no normal publish projection. Publish,
resume, direct parallel dispatch, rehearsal and clean share the same retained
hold check; durable status and watch expose false resume safety and investigation
guidance. No automatic hold-clear operation is defined.

A complete typed hold event still blocks cleanup when a different event line
is malformed. This positive hold scan cannot authorize publication or rewrite
history; corrupt evidence without a recognized hold still blocks publish and
resume. If the hold record itself is unreadable or unrecognizable, explicit
clean retains its ordinary corrupt-artifact recovery behavior.

For a multi-registry `clean`, Shipper acquires ownership and checks the base
state directory and every selected registry directory before deleting state,
events, receipts, reconciliation reports or caches anywhere in that selection.
A recognized cleanup hold or an unavailable ownership key in any selected
directory refuses the whole deletion pass, including with `--force` or
`--keep-receipt`. All acquired owners remain held through the deletion pass.
This does not make deletion transactional: a filesystem error during the later
deletion pass can still leave a partial clean.

Events remain authoritative, but a held projection without its matching event
is evidence of a failed persistence boundary, not authorization to retry.
`rebuild_state_file_from_events` requires an offline directory or exclusive
caller ownership of the publication key for its whole call. It has no workspace
or ownership input and does not synchronize against concurrent publishers or
arbitrary embedding writes. With that precondition, it refuses to overwrite an
already-retained held projection from an older event prefix; it neither
fabricates the missing event nor clears the hold. Within a replay, a hold event
always survives later
ordinary retry, success or reconciliation events. The persistence limits and
platform scope are described in [failure modes](../failure-modes.md#retained-process-cleanup-hold).

### `state.json`

```json
{
  "state_version": "...",
  "plan_id": "23ff8f85...",
  "registry": {"name": "crates-io", "api_base": "https://crates.io"},
  "attempt_history": [
    {
      "package": "shipper-types",
      "version": "0.3.0-rc.1",
      "attempt": 1,
      "max_attempts": 3,
      "started_at": "...",
      "ended_at": "...",
      "error_class": "retryable",
      "next_attempt_at": "...",
      "redacted_message": "rate limited"
    }
  ],
  "packages": {
    "shipper-types@0.3.0-rc.1": {
      "name": "shipper-types",
      "version": "0.3.0-rc.1",
      "attempts": 1,
      "state": {"state": "published"},
      "last_updated_at": "..."
    }
  }
}
```

**Field path caveat**: package state lives at `.packages[].state.state` (nested), **not** `.packages[].status`. Common misread.

For a nonempty rebuild, top-level `created_at` and `updated_at` use the first
and last appended event timestamps respectively, not the minimum and maximum.
An empty rebuild with a fallback plan ID uses rebuild time for both. Package
`last_updated_at` uses the last appended event that updates that projection.
Ordinary live transitions sample projection update time separately; cleanup-hold
persistence fallback can retain the observed attempt end instead. These
wall-clock fields need not increase, and rebuilt `updated_at` can precede
`created_at`.

`plan_id` binds the registry API base and ordered package names/versions. It
does not bind source bytes, workspace path, or a unique execution. Resume uses
it as one guard, not as provenance for the checkout restored by an operator.

Affirmative lock-holder liveness currently depends on Linux `/proc` process
identity. Unsupported platforms, legacy locks, missing/corrupt locks, and
cross-host evidence remain `Unknown`; PID or lock age alone is not equivalent
proof.

`attempt_history` is the per-attempt projection replayed from `events.jsonl` and is used by diagnostics and recovery workflows after interruption.
It is not guaranteed to be timestamp-sorted. `started_at` retains the original
start observation. `ended_at` follows the live or replay completion path without
clamping a backward clock: a retry decision preserves an earlier failure end,
but replaying `StillUnknown` can replace it with the reconciliation event time.
The live executor retains the earlier failure end on that path, so live and
rebuilt values are not universally identical. An unfinished attempt without an
end observation retains its start time in `ended_at`.

Per-attempt recovery fields are under `.attempt_history[]`, keyed by:

- `package`, `version`
- `attempt`, `max_attempts`
- `started_at`, `ended_at`
- `error_class`, `next_attempt_at`, `redacted_message`

### `receipt.json`

```json
{
  "receipt_version": "shipper.receipt.v2",
  "plan_id": "...",
  "registry": {...},
  "started_at": "...",
  "finished_at": "...",
  "packages": [
    {
      "name": "shipper-types",
      "version": "0.3.0-rc.1",
      "attempts": 1,
      "state": {"state": "published"},
      "started_at": "...",
      "finished_at": "...",
      "duration_ms": 3400,
      "evidence": {...}
    }
  ],
  "event_log_path": ".shipper/events.jsonl",
  "git_context": {...},
  "environment": {...},
  "execution_result": "success"
}
```

`git_context` describes the selected workspace's repository at the start of
the publish or resume invocation. Its commit, branch, tag, and dirty flag do
not come from the caller's working directory when `--manifest-path` selects
another workspace. Unavailable target Git evidence stays absent or unknown;
Shipper does not substitute the caller's repository.
Both evidence collection and the cleanliness gate ignore inherited Git
repository-location, object-store, reference-view, and index overrides such as
`GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, and `GIT_INDEX_FILE`. Git
configuration, authentication settings, and `SHIPPER_GIT_BIN` remain available.

`execution_result` is the aggregate receipt outcome: `"success"`,
`"partial_failure"`, or the compatibility-retained `"complete_failure"` value.
Current finalized publish/resume runs emit `success` (exit `0`) or
`partial_failure` (exit `2`); `complete_failure` is not currently reachable
from receipt finalization. Exit `1` means the command errored before a receipt
was finalized, so do not infer it from `receipt.json`. The field is
`#[serde(default)]` — receipts written before it existed deserialize as
`"success"`.

## jq one-liners

```bash
# All packages that published successfully
jq -r 'select(.event_type.type == "package_published") | .package' .shipper/events.jsonl | sort -u

# Last durable event (this does not prove that the process is live)
jq -c '.' .shipper/events.jsonl | tail -1

# Package states from state.json
jq -r '.packages[] | "\(.name): \(.state.state)"' .shipper/state.json

# Plan ID for comparison across runs
jq -r '.plan_id' .shipper/state.json

# Reconciliation outcomes
jq -c 'select(.event_type.type == "publish_reconciled") | .event_type' .shipper/events.jsonl

# Drift (should be empty on a healthy run)
jq -c 'select(.event_type.type == "state_event_drift_detected")' .shipper/events.jsonl
```

## See also

- [INVARIANTS.md](../INVARIANTS.md) — truth/projection/summary contract (normative)
- [how-to/inspect-a-stalled-run.md](../how-to/inspect-a-stalled-run.md) — triage recipes
- [how-to/inspect-state-and-receipts.md](../how-to/inspect-state-and-receipts.md) — post-run inspection
- [explanation/why-shipper.md](../explanation/why-shipper.md) — why the three-file split exists
