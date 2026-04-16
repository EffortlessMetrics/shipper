# Invariants

## Truth, projection, summary

Three files in `.shipper/` carry execution data. They have a strict ordering of authority.

### `events.jsonl` — truth
Append-only. Every state transition emits exactly one event with timestamp, package context, and event-type-specific payload. Never replayed-and-rewritten. Never compacted (events are bounded by package count × attempt count, both small).

### `state.json` — projection
A serialized view of the current `ExecutionState`, rewritten after every package state change. Equivalent to a fold over `events.jsonl` followed by snapshotting. Used by `shipper resume` for fast recovery without replaying the full event log.

### `receipt.json` — summary
Written once at end-of-run. Summarizes packages, plan, registry, environment, and event log location. Intended for CI artifacts and audit consumers.

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
