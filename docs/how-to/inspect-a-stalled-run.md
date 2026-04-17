# How to inspect a stalled or interrupted run

Your CI log has been quiet for 10 minutes. Or the workflow got cancelled mid-publish. Or you're trying to figure out whether the train is still alive or genuinely hung. This guide is the triage path.

> **Related:** [Inspect state, events, and receipts](inspect-state-and-receipts.md) covers the "what happened after the run" case. This doc is about the "is it alive right now?" and "what will resume do?" cases.

## Triage: which file answers which question?

| Question | File | Command |
|---|---|---|
| Is the train alive or hung? | `events.jsonl` (latest entries) | `tail -n 20 .shipper/events.jsonl \| jq -c '.'` |
| What's the current crate? | `events.jsonl` (last `package_started`) | see below |
| How long has it been waiting? | `events.jsonl` (last `retry_backoff_started`) | see below |
| Which crates finished? | `events.jsonl` (published events) OR `state.json` | see below |
| What's next when I resume? | `state.json` (packages with `state.state == "pending"`) | see below |
| Why did it fail? | `events.jsonl` (last `package_failed` / `publish_reconciled.StillUnknown`) | see below |

**Authority order**: `events.jsonl` > `state.json` > `receipt.json`. When they disagree, events win. Per [INVARIANTS.md](../INVARIANTS.md).

## "Is it alive?" — the 30-second check

```bash
# What was the last event, and how recent?
jq -c '.' .shipper/events.jsonl | tail -1
```

If the last event is a `retry_backoff_started`, the run is alive and waiting. Check `next_attempt_at`:

```bash
jq -c 'select(.event_type.type == "retry_backoff_started") | .event_type' .shipper/events.jsonl | tail -1
```

Expected output includes `next_attempt_at` (an ISO-8601 timestamp). If it's still in the future, the train is alive and scheduled to retry. If it's in the past by more than a few minutes, something may actually be stuck.

```bash
# Which crate are we currently working on?
jq -r 'select(.event_type.type == "package_started") | .package' .shipper/events.jsonl | tail -1
```

## "What's normal waiting?"

crates.io's publish rate limits:

- **Brand-new crates**: 5-crate burst, then 1 new crate per 10 minutes
- **New versions of existing crates**: 30 per minute after a 30-burst

If you're publishing 12 new crates, expect ~90-minute wall clock. Silence between retries of up to ~10 minutes is **normal**. If it's been 30+ minutes since the last event, investigate.

`retry_backoff_started` events (added in [#91](https://github.com/EffortlessMetrics/shipper/issues/91)) now carry the full context — reason, attempt number, next-attempt time. A run that's correctly waiting on crates.io will emit these regularly.

## "What did already publish successfully?"

```bash
# From events.jsonl (authoritative)
jq -r 'select(.event_type.type == "package_published") | .package' .shipper/events.jsonl | sort -u

# From state.json (projection, fast)
jq -r '.packages[] | select(.state.state == "published") | .name' .shipper/state.json | sort -u
```

If these two lists disagree, events are truth. A mismatch triggers `StateEventDriftDetected` at end-of-run (added in [#93](https://github.com/EffortlessMetrics/shipper/issues/93)).

## "What will resume do?"

`shipper resume` reads `state.json`, validates the `plan_id` matches the current workspace, and continues from the first non-terminal package. Terminal states for resume: `Published`, `Skipped`. Non-terminal: `Pending`, `Failed`, `Ambiguous`.

```bash
# What's pending?
jq -r '.packages[] | select(.state.state == "pending") | .name' .shipper/state.json

# Anything ambiguous that will trigger resume-time reconciliation?
jq -r '.packages[] | select(.state.state == "ambiguous") | "\(.name): \(.state.message)"' .shipper/state.json

# Anything failed?
jq -r '.packages[] | select(.state.state == "failed") | "\(.name): \(.state.message)"' .shipper/state.json
```

On `Ambiguous` state, resume will reconcile against the registry *before* any cargo activity — see [why-shipper.md](../explanation/why-shipper.md#cargo-stdout-is-a-hint-the-registry-is-the-truth).

## "Why did it fail?"

```bash
# Last failure event with full context
jq -c 'select(.event_type.type == "package_failed") | .event_type' .shipper/events.jsonl | tail -1

# Reconciliation outcomes (in-flight or resume-time)
jq -c 'select(.event_type.type == "publish_reconciled") | .event_type.outcome' .shipper/events.jsonl
```

If you see `"outcome": {"outcome": "still_unknown"}`, the registry couldn't resolve the ambiguous publish — this is the one case where operator judgment is required. The `reason` field tells you what the reconciliation query errored with.

## Common scenarios

### Scenario: workflow was cancelled mid-publish

1. Download the `shipper-state-final` artifact from the cancelled run (or `shipper-state-preflight` / `shipper-state-plan` if later stages never ran).
2. Trigger the `release-resume` workflow_dispatch with `mode=resume` and `artifact_run_id=<cancelled-run-id>`.
3. The resume job downloads the artifact, runs `shipper resume`, and continues.

### Scenario: runner timed out after 6 hours

Same procedure. The `.shipper/` artifact was uploaded at each stage (plan / preflight / final) so even a mid-publish timeout leaves the most recent state available.

### Scenario: "how do I know it's really done?"

Once the workflow completes successfully, the receipt is the audit artifact:

```bash
jq '.' .shipper/receipt.json | head -40
```

Per-package state, timestamps, attempt counts, and evidence (captured stdout/stderr, registry readiness checks) all land here. Keep it for audit.

## See also

- [INVARIANTS.md](../INVARIANTS.md) — the truth/projection/summary contract
- [inspect-state-and-receipts.md](inspect-state-and-receipts.md) — post-hoc inspection (what happened)
- [state-files.md](../reference/state-files.md) — one-page cheat sheet
- [Tutorial: Recover from an interrupted release](../tutorials/recover-from-interruption.md) — end-to-end walkthrough
