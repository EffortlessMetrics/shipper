# Invariants

## Truth, projection, summary

Three files in `.shipper/` carry execution data. They have a strict ordering of authority.

### `events.jsonl` — truth
Append-only. Every state transition emits exactly one event with timestamp, package context, and event-type-specific payload. Never replayed-and-rewritten. Never compacted (events are bounded by package count × attempt count, both small).

### `state.json` — projection
A serialized view of the current `ExecutionState`, rewritten after every package state change. Equivalent to a fold over `events.jsonl` followed by snapshotting. Used by `shipper resume` for fast recovery without replaying the full event log.

### `receipt.json` — summary
Written once at end-of-run. Summarizes packages, plan, registry, environment, and event log location. Intended for CI artifacts and audit consumers.

## Event order and wall-clock time

**Append order is authoritative.** Replay reads each JSONL file in line order.
The writer, reader, package filter, and `shipper inspect-events` preserve that
order. `inspect-events --follow` continues from the appended tail; it does not
merge events by timestamp. Separate preflight audit logs remain separate logs.

**Each package's causal sequence is recorded in append order.** An attempt
precedes its outcome, and a retry decision precedes the next attempt. Workers
for different packages may interleave. A worker can observe an event, wait for
the shared event-log mutex, and append it after another worker's newer
observation. `EventLog` preserves the supplied sequence; it does not repair an
invalid causal sequence supplied by an embedding caller.

**Timestamps are original wall-clock observations.** They are not sequence
numbers. Delayed appends can invert timestamps across packages, and clock
rollback can invert them within a package. Do not sort, clamp, or replace
timestamps to reconstruct execution. JSON output preserves the recorded
timestamp values and precision; human text may format them for display.

For a nonempty event log, rebuilt `ExecutionState.created_at` is the first
appended event's timestamp, and rebuilt `updated_at` is the last appended
event's timestamp. They are **not** the minimum and maximum timestamps.
`updated_at` can precede `created_at`. An empty rebuild with a fallback plan ID
uses the rebuild time for both. A rebuilt package's `last_updated_at` comes from
the last appended event that updates that package's projection. Live state
creation and ordinary transitions sample the wall clock at projection update.
Cleanup-hold persistence fallback can retain the observed attempt end time
instead. Rebuilding does not promise identical live projection metadata
timestamps.

Attempt timestamps follow the existing completion semantics without clock
normalization: a recorded failure can end before its attempt's start after clock
rollback, and a later retry decision does not move that end time. For journals without an upload checkpoint, replay can use the
`StillUnknown` reconciliation time as the attempt end. A modern upload
checkpoint retains the Cargo end time and classification while reconciliation
updates package progress. Use package and attempt identity to interpret
causality.

A checkpointed attempt keeps the history slot of its first durable detail.
Later reconciliation and retry events refine the same slot in live state and
replay. Other completed attempt details follow replay finalization, and open
attempts without a checkpoint keep the deterministic fallback ordering at the
end of replay. The history array need not be globally chronological.

## The invariant

> The set of `package_published` events in `events.jsonl` MUST equal the set of packages with `state.state == "published"` in `state.json`.

Drift between events and state is a bug. Per [#93](https://github.com/EffortlessMetrics/shipper/issues/93), an end-of-run consistency check enforces this and emits `state_event_drift_detected` on mismatch.

## Why it matters

`shipper resume` reads `state.json` to decide which packages to skip. If state.json drifts from events, resume could either re-publish duplicates (state under-reports success) or refuse to continue valid work (state over-reports success).

The contract guarantees: even if `state.json` is corrupted or deleted, the run can be reconstructed from `events.jsonl` alone (per [#101](https://github.com/EffortlessMetrics/shipper/issues/101)'s state-rebuild capability).

## Field-name caveat

In `state.json`, package status is at `.packages[].state.state`, not `.packages[].status`. The original v0.3.0-rc.1 retrospective briefly misread the file by querying the wrong path. The invariant above is what makes that ambiguity recoverable: events are the truth, the projection's exact field path is implementation detail.

## Practical guidance for tooling

If you are writing tooling that consumes Shipper output:

- **For per-event audit / streaming**: read `events.jsonl`. Each line is one JSON object.
- **For "what's the current state"**: read `state.json`. Treat it as a cache; reconcile against events if you suspect drift.
- **For "did this release succeed and what was published"**: read `receipt.json`.

Never derive critical decisions from CLI stdout alone. Stdout is a human-facing rendering of the events; structured consumers should always go to the JSON files.

## Uploaded recovery checkpoint

`EventType::PackageUploaded` is emitted only after Cargo has accepted an
upload. It is now the durable checkpoint for `PackageState::Uploaded`:
state rebuild maps that event to `uploaded`, and a later `package_published`
event advances the projection to `published`.

For historical logs, replay infers `uploaded` from `EventType::ReadinessStarted`
only when the package's current `PackageAttempted` has no recorded attempt
ceiling, a `PackageOutput` follows it, and the active attempt in replay still
matches. A modern diagnostic registry visibility probe alone cannot establish
Cargo acceptance.

## Persistence boundary and failure model

The filesystem path used by ordinary publish/resume requires a successful
`sync_all` of the event file after flushing each buffered JSONL batch. Only then
may a package transition clear its pending event batch and write its state
projection. Plaintext JSON replacements (`state.json`, `receipt.json`,
`reconciliation.json`, and remediation plans) require the temporary file's
write and `sync_all` to succeed before renaming it over the destination.
Required file-sync errors retain the affected path and underlying I/O error.
Cleanup persistence refusals retain both the typed cleanup stop and that I/O
cause; a successful projection fallback does not turn the sync failure into success.
The first returned error keeps its original source chain. After an attempted
append, a write/flush/sync error makes that `EventLog` instance refuse all later
writes, including writes to another path. It preserves pending events even when
`clear()` is called. Later refusals retain the original diagnostic and reconstruct
the I/O error kind, raw OS code (when present), and message; arbitrary custom
source types remain available only on the first error. Directory-creation and
file-open failures before any append can still be retried.

This is a checked file-sync boundary, not an atomic transaction across files or
between local files and a registry. In particular:

| Failure or interruption | Supported result and limit |
|---|---|
| Returned event write, flush, or sync error | The transition returns an error before advancing its projection. The same event-log owner refuses later appends and retains pending evidence, so a peer or readiness close cannot blindly replay the failed batch. Already-appended bytes remain; an error does not prove that the event or the remote upload is absent. |
| Returned temporary JSON write or sync error | Rename is not attempted. An existing destination remains readable and unchanged, or a missing destination stays missing. The temporary file may remain for diagnosis. |
| Projection failure after event sync | The synchronized event remains authoritative while the old projection can lag. Rebuild can recover from readable complete events; no rollback or fabricated replacement event is performed. |
| Receipt sync failure | Finalization returns an error and does not install a new receipt. A prior receipt, if any, remains prior evidence. `ExecutionFinished` may already be present: that event does not prove the command returned success or installed its receipt. |
| Abrupt Shipper process termination | Recovery uses the evidence actually retained. Termination during append may leave an incomplete final JSONL record or a projection behind its events. The checked boundary does not make each append atomic or guarantee automatic recovery at every instruction. |
| Runner loss and artifact transfer | A successful rehearsal proves continuation from the artifacts that were successfully retained and transferred. It does not prove that an abruptly lost runner uploaded its newest bytes. |
| Host crash, power loss, filesystem or controller failure | No certification is made. A successful file-sync call is the OS-reported boundary, not independent proof of hardware persistence or atomic multi-file recovery. |

Parent-directory synchronization after JSON rename remains **best effort**:
open/sync errors are ignored for platform portability. Event-file or directory
creation is not followed by a required directory-sync barrier. File sync alone
therefore does not guarantee the persistence of a new name or a rename across
host loss. Required file-sync errors are not downgraded to this directory policy.

The separate `save_state_encrypted` / `write_receipt_encrypted` helpers delegate
to `shipper-encrypt`'s direct-write implementation, even when encryption is
disabled in that helper. They do not use the atomic JSON writer and are outside
this bounded replacement guarantee. Their behavior and support tier are not
promoted by these tests.

After a sync error, preserve the event log and inspect the artifacts and registry
truth before continuing; do not delete appended bytes or blindly append the same
pending batch again. The failed-owner hold is in memory: constructing a new log
or restarting does not retain it, and it does not cancel already-running peer
Cargo processes. Strict event readers still reject incomplete, malformed,
and unknown records. Follow-mode buffering is a separate streaming behavior and
does not authorize silently repairing authoritative history.

The deterministic `file_sync` tests inject a file-sync error only after reading
the actual flushed event or written temporary JSON. They prove error propagation,
event-before-projection ordering, preservation before rename, rebuild after a
projection failure, and no success receipt for the selected failure points in
both schedulers. A two-worker fault also checks the actual event count and
independently rebuilt attempt history; public resume and readiness-close faults
exercise contextual error forwarding and refusal of a repeated append. Native
proof is Linux-only; no macOS, Windows, filesystem,
controller-cache, or real power-loss qualification follows from fault injection.

## Process timeout enters reconciliation

The canonical package executor treats `CargoOutput::timed_out` as
`ErrorClass::Ambiguous` before inspecting output text. Killing Cargo cannot
establish whether the registry accepted the upload. This holds when retained
tails contain Retryable/Permanent patterns and when the configured tail is
empty. Both schedulers use this decision and the same three-way registry
reconciliation. Ordinary non-timeout Cargo exits with complete tails retain their output-based
classification. This rule does not change process capture, memory limits, or
descendant cleanup guarantees.

## Publish diagnostic capture is bounded and loss is explicit

Cargo publication, including rehearsal, opts into bounded diagnostic retention.
Each stream retains at most 256 KiB of payload, a 64 KiB partial line, and 4096
line entries while reading fixed 8 KiB chunks. Normal eviction outside the
requested last-N-lines window is intentional tailing. Capacity or read loss is
sticky: both diagnostic streams are suppressed in favor of a fixed omission
summary, so discarded authorization context cannot leave a credential suffix or
continuation in evidence. UTF-8 decoding and redaction also obey the final payload
limit. `output_lines=0` intentionally returns empty tails; it is not capacity loss.

The canonical executor consumes typed completeness, not the summary text. An
unsuccessful incomplete result enters Ambiguous reconciliation after the typed
timeout check. Cargo success still establishes Uploaded and proceeds through
readiness. Attempt counts and registry outcome authority do not change. Rehearsal
reports incomplete failure as Ambiguous without adding a retry policy. Public
CargoOutput fields remain compatible.

Generic complete-output capture and structured metadata remain complete. Yank,
install and dry-run capture are unchanged. This contract bounds retained publish
diagnostics, not descendant lifetimes or inherited-pipe EOF; cleanup bounds remain
separate acceptance in #412.

## Readiness evidence matches readiness events

`ReadinessEvidence.delay_before` is the delay that was **actually slept**
immediately before that poll — not an independently recomputed backoff
sample. It equals the `delay_ms` of the `ReadinessPollScheduled` event that
announced the same attempt, so the receipt's readiness evidence and the event
log can never disagree about how long a run waited.

This holds for the first attempt too: when `readiness.initial_delay` is
configured, the loop emits `ReadinessPollScheduled { attempt: 1 }` and sleeps,
and attempt 1's evidence reports that same `initial_delay`. It reports
`0` only when nothing was slept before the first poll.

There is exactly one implementation of this loop —
`shipper_registry::RegistryClient::is_version_visible_with_backoff_and_events`
(see [#202](https://github.com/EffortlessMetrics/shipper/issues/202)). The
engine wraps it with the `ReadinessStarted` / `ReadinessComplete` /
`ReadinessTimeout` / `ReadinessError` envelope but does not re-implement the
polling or the evidence. `ReadinessTimeout` means the wait budget ended with a
conclusive negative observation. `ReadinessError` means the result is
inconclusive or polling aborted, and records elapsed time rather than the
configured budget. An inconclusive final probe can therefore produce
`ReadinessError` at budget exhaustion as well as before it.

### Unknown observations cannot authorize another upload

A positive API or valid sparse-index observation establishes publication.
With readiness enabled, negative reconciliation evidence requires a successful
final observation from every configured authority: API `404`, sparse-index `404`, or a fully readable
index containing valid Cargo semantic versions but not the requested version. In
`Both` mode both probes must establish absence in the same final poll;
`prefer_index` changes query order only. Either probe may still establish
presence while the other is unavailable. Disabled readiness retains its legacy
single API observation, irrespective of the configured polling method.

Connection errors, request timeouts, authentication/rate-limit/server errors,
empty or malformed index responses, an unreadable local index, and a `304`
without a sent validator and readable cached content remain unknown. A
configured local index is an explicitly selected observation source; Shipper
does not independently prove that an operator-managed mirror is fresh. Cached
HTTP content is subject to the same parsing rules. A cached positive record
can confirm presence. A `304` with no valid positive record triggers one
unconditional refresh before absence can be claimed: cached bytes and ETags
are stored separately and a partial cache write must not authorize a retry.

The polling loop may recover from transient errors. If its final poll is
unknown, an earlier negative poll cannot authorize `NotPublished`: the engine
records `StillUnknown`, retains ambiguous state, and stops before another
Cargo invocation. Failed probes are not emitted as `ReadinessPoll(false)` or
stored as negative `ReadinessEvidence`. Conclusive evidence keeps its actual
poll number, so gaps in attempt numbers are expected after failed probes.

The public polling signatures remain `Result<(bool, Vec<ReadinessEvidence>)>`;
the additive `shipper_registry::ReadinessObservationError` retains the total
poll count and earlier conclusive evidence for consumers that downcast it.
`check_index_visibility` now returns errors for unknown observations rather
than collapsing them to `false`. No event or artifact schema changes are
required. The permissive `shipper_sparse_index::contains_version` search helper
retains its existing contract; registry observations use `observe_version`.

Attempt details are appended to `state.json` through the same event-first
transition boundary as the matching terminal, reconciliation, or retry event.
This prevents a scheduler from independently persisting the attempt timeline
and package state. State rebuild now projects all event-vocabulary-carrying
attempt metadata into `attempt_history`, including retry backoff timestamps.
Remaining edge cases are only where an event cannot describe the same detail
that a runtime-only attempt transition could infer locally.

Parallel package workers own the explanatory events for a transition until its
projection is ready. The canonical executor validates the entire package batch,
then holds the shared event-log mutex across appending those events and the
event-first state/attempt-detail commit. Another worker may append or flush
before that boundary, but cannot replace its pending tail during the commit.
Lock ordering is event log, then execution state; Cargo, registry queries,
reporting and retry sleeps remain outside that critical section. This is an
in-process concurrency guarantee, not an all-or-nothing filesystem transaction
or a host/power-loss durability claim.

Reconciliation report generation uses that same event-log mutex across the
complete durable-event read, record derivation, and report replacement. Package
workers cannot overwrite newer reconciliation evidence with an older snapshot
or interfere with another worker's `reconciliation.tmp`. The report path takes
only the event-log mutex; package transitions retain the event-log-then-state
lock order. Cargo, registry polling, backoff, and report warnings remain outside
this report persistence boundary.

Post-join finalization derives the report after workers finish, while the
invocation still owns its publication key. A failed report read or write remains
observable, and the complete event/state/report consistency check owns recovery
authorization. A worker's local result cannot substitute for that check. The
low-level report writer requires caller-provided exclusion for its whole call;
this is an engine concurrency guarantee, not a new generic storage transaction,
cross-process locking primitive, or fsync/power-loss guarantee.

### Local process cleanup holds

`package_cleanup_incomplete` records a local safety condition that takes
precedence over registry observations. Its `cleanup_incomplete` projection
cannot authorize another upload. Attempt admission checks every package hold
under the same mutex used to publish an observed hold; already-admitted peers
may finish, but later attempts and retries are refused. Rehearsal participates
in the same invocation ownership key and checks retained evidence again after
reservation.

An observed hold is installed in memory before event/state persistence. If its
event cannot be appended, the existing state writer may retain a projection-only
hold as explicit evidence of that failed boundary. This is not an invented
authoritative event or a normally consistent projection. Publish/resume refuse
either retained form. An offline or exclusively invoked rebuild must refuse to
erase an already-retained projection-only hold; the low-level rebuild API does
not synchronize concurrent embedding writes. Within any replay, a hold event is
absorbing despite later ordinary package events. Both-write failure, rehearsal
hold-event append failure and crashes before hold persistence have no new
cross-invocation durability guarantee. See [failure modes](failure-modes.md#retained-process-cleanup-hold)
for the Unix containment and compatibility limits.

## Artifact compatibility across the 0.4 to 0.5 line

The compatibility promise is asymmetric:

- **0.5 artifacts:** new events contain enough information to rebuild every
  field the project claims is reconstructible. State and receipt fields are
  projections/summaries, not a second source of truth.
- **0.4 artifacts:** old event, state, and receipt files remain readable and
  safely resumable. A field absent from the old vocabulary is `unknown`; it
  must not be fabricated as zero or inferred without evidence.

Encrypted payloads use a versioned KDF header for new writes. KDF v1
unversioned payloads remain readable at their legacy PBKDF2 cost, while KDF v2
is the new stronger default. Unsupported or unreasonable parameters are
rejected rather than accepted as an implicit fallback.

Security evidence follows the same non-disclosure rule as execution evidence:
complete authorization header values and bounded `Token=`/`ApiKey=` values are
redacted, ordinary prose is preserved, and webhook configuration secrets do
not appear in events, state, receipts, diagnostics, structured output, or
logs. OIDC availability requires both request variables to be present and
nonblank; blank diagnostics never include their values.

## Containment effects and local evidence

Single yank, yank-plan execution, and guarded remediation prepare their shared
event destination before invoking Cargo. Each observed result must append its
`PackageYanked` event before the command counts success or starts another yank.
A returned append error stops the remaining plan and produces a nonzero,
essential diagnostic even under `--quiet`. The diagnostic preserves the
observed Cargo result without treating missing local evidence as proof that no
registry effect occurred.

A requested receipt amendment is checked against the selected registry and
package before the effect and re-read after the event is recorded. Failure to
load, match, or write that amendment is an incomplete operation, not a successful
containment command. Existing event bytes are preserved. Direct and yank-plan
reasons retain their intentional
recording contract; guarded remediation retains its redacted reason placeholder.

These checks handle ordinary returned filesystem errors. They do not make
preflight a transaction or add a host/power-loss durability guarantee. An
incomplete-evidence diagnostic calls for registry and artifact reconciliation,
not an automatic repeat of the remote operation.
