# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

# Module: `crate::state::events`

**Layer:** state (layer 3)
**Single responsibility:** Append-only JSONL event log for publish operations.
**Was:** standalone crate `shipper-events` (physically absorbed in PR #60 shim +
physical move)

## Public-to-crate API

- `EventLog` — in-memory append-only event log
- `EVENTS_FILE` — canonical event file name (`events.jsonl`)
- `events_path(state_dir)` — helper to build `<state_dir>/events.jsonl`

## Status

Physically absorbed: the full implementation lives in `mod.rs` (production
code), `tests.rs` (unit + snapshot tests), and `proptests.rs` (property-based
tests). Snapshots live in `snapshots/`. The standalone `shipper-events` crate
has been deleted from the workspace.

## Invariants

- Append-only: events are never deleted or reordered.
- One event per JSON object per line.
- Persistence success requires buffered JSONL flush and underlying file sync; an error may leave appended bytes and never authorizes deleting or blindly reappending them.
- An attempted append/flush/sync failure holds that `EventLog` instance against later writes, including another path and after `clear()`. Keep pending evidence and the first error intact; subsequent refusals retain its diagnostic and I/O metadata. Pre-append create/open failures may be retried. This is not a persisted cross-invocation hold.
- Strict readers reject incomplete, malformed, and unknown events; do not silently repair authoritative history. Parent-directory durability is not guaranteed.

