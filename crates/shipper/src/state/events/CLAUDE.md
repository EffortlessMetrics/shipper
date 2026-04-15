# Module: `crate::state::events`

**Layer:** state (layer 3)
**Single responsibility:** Append-only JSONL event log for publish operations.
**Was:** standalone crate `shipper-events` (partial absorption in this PR)

## Public-to-crate API

- `EventLog` — in-memory append-only event log
- `EVENTS_FILE` — canonical event file name (`events.jsonl`)
- `events_path(state_dir)` — helper to build `<state_dir>/events.jsonl`

## Status

This module is the canonical path (`crate::state::events::X`) that all internal
and CLI code now uses. The implementation currently re-exports from the
standalone `shipper-events` crate because `shipper-store` and
`shipper-engine-parallel` still depend on it via their own `shipper_events`
path-dep — changing that would require absorbing those crates too. When they
are absorbed in a future PR, the full implementation (currently at
`crates/shipper-events/src/lib.rs`) will move here and `shipper-events` will
be deleted from the workspace.

## Invariants

- Append-only: events are never deleted or reordered.
- One event per JSON object per line.
- File format is forward-compatible — readers ignore unknown event types.
