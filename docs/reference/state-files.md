# State files reference — `.shipper/`

One-page cheat sheet. For the full contract see [INVARIANTS.md](../INVARIANTS.md); for triage recipes see [inspect-a-stalled-run.md](../how-to/inspect-a-stalled-run.md).

## Authority order

**`events.jsonl` > `state.json` > `receipt.json`**

When they disagree, events win. `state.json` and `receipt.json` are projections/summaries derived from events. An end-of-run consistency check emits `StateEventDriftDetected` if drift is found.

## Per-file summary

| File | Authority | Purpose | When written | Format |
|---|---|---|---|---|
| `events.jsonl` | **Truth** (append-only) | Every state transition with timestamp | Per event | JSONL (one event per line) |
| `state.json` | Projection | Serialized `ExecutionState` for fast resume | Per package state change | JSON |
| `receipt.json` | Summary | End-of-run audit artifact with evidence | Once, at run completion | JSON |
| `lock` | — | Concurrent-publish guard | Held during the run | Small text file |

## Which file for which question?

| Question | File |
|---|---|
| What happened, in order? | `events.jsonl` |
| What's the current state (fast lookup)? | `state.json` |
| Did the whole release succeed, and what's the audit trail? | `receipt.json` |
| What would `shipper resume` skip? | `state.json` (packages with `state.state == "published"`) |
| What's the truth when they disagree? | `events.jsonl` |

## Key field paths

### `events.jsonl` (one JSON object per line)

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
- `package_started`, `package_attempted`, `package_published`, `package_failed`, `package_skipped`
- `retry_backoff_started` — added in [#91](https://github.com/EffortlessMetrics/shipper/issues/91); carries attempt N/M, delay, reason, next-attempt time
- `publish_reconciling`, `publish_reconciled` — added in [#99](https://github.com/EffortlessMetrics/shipper/issues/99); registry-truth resolution of ambiguous outcomes
- `state_event_drift_detected` — added in [#93](https://github.com/EffortlessMetrics/shipper/issues/93); end-of-run consistency check
- `execution_started`, `execution_finished`

### `state.json`

```json
{
  "state_version": "...",
  "plan_id": "23ff8f85...",
  "registry": {"name": "crates-io", "api_base": "https://crates.io"},
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
  "environment": {...}
}
```

## jq one-liners

```bash
# All packages that published successfully
jq -r 'select(.event_type.type == "package_published") | .package' .shipper/events.jsonl | sort -u

# Last event (is the run alive?)
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

## Sidecar files

Depending on what ran, `.shipper/` may also contain:

| File | Produced by | Contents |
|---|---|---|
| `preflight_workspace_verify.txt` | Preflight, when workspace dry-run ran | Full ANSI-stripped cargo output ([#92](https://github.com/EffortlessMetrics/shipper/issues/92)) |
| `plan.txt` | `shipper plan --format json` (with tee) | Plan JSON for artifact inspection |

## See also

- [INVARIANTS.md](../INVARIANTS.md) — truth/projection/summary contract (normative)
- [how-to/inspect-a-stalled-run.md](../how-to/inspect-a-stalled-run.md) — triage recipes
- [how-to/inspect-state-and-receipts.md](../how-to/inspect-state-and-receipts.md) — post-run inspection
- [explanation/why-shipper.md](../explanation/why-shipper.md) — why the three-file split exists
