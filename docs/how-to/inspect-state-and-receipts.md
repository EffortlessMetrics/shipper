# How to inspect Shipper state, events, and receipts

After a Shipper run, three files in `.shipper/` tell the story. Different questions → different file.

## Which file for which question?

| Question | File | Command |
|---|---|---|
| Exactly what happened, and when? | `events.jsonl` | `shipper inspect-events` |
| What's the current state (for resume)? | `state.json` | `cat .shipper/state.json` |
| What was the final outcome + evidence? | `receipt.json` | `shipper inspect-receipt` |

**Authority order:** events are truth, state is a projection, receipt is a summary. When they disagree, events win. See [INVARIANTS.md](../INVARIANTS.md).

## Reading events

```bash
shipper inspect-events
```

Or raw:

```bash
jq -c '.' .shipper/events.jsonl
```

Each line is one JSON event with:
- `timestamp` — RFC3339 UTC
- `event_type.type` — variant name (`package_started`, `package_published`, `preflight_complete`, etc.)
- `package` — "all" or "<name>@<version>"
- Event-specific payload under `event_type.*`

### Count events by type

```bash
jq -r '.event_type.type' .shipper/events.jsonl | sort | uniq -c | sort -rn
```

### Extract just the publish outcomes

```bash
jq -c 'select(.event_type.type == "package_published")' .shipper/events.jsonl
```

### See every attempt (including retries)

```bash
jq -c 'select(.event_type.type == "package_attempted")' .shipper/events.jsonl | head -20
```

## Reading state

```bash
jq '.' .shipper/state.json
```

Key field: `packages[].state.state` (yes, nested) — values include `"pending"`, `"published"`, `"failed"`, `"ambiguous"`. **Not** `.packages[].status` — that field doesn't exist (common misread).

```bash
# Packages that are published
jq '.packages[] | select(.state.state == "published") | .name' .shipper/state.json

# Packages still pending
jq '.packages[] | select(.state.state == "pending") | .name' .shipper/state.json
```

## Reading receipts

```bash
shipper inspect-receipt
```

Or JSON for CI consumption:

```bash
shipper inspect-receipt --format json | jq '.'
```

The receipt is the audit artifact. Keep it. It includes `plan_id`, per-package outcomes, captured evidence (stdout/stderr tails + exit codes), git context, and an environment fingerprint.

## Verifying consistency

If you want to sanity-check that events and state agree:

```bash
# package_published events
jq -r 'select(.event_type.type == "package_published") | .event_type.crate_name' .shipper/events.jsonl | sort > /tmp/events_published.txt

# packages with state.state == "published"
jq -r '.packages[] | select(.state.state == "published") | .name' .shipper/state.json | sort > /tmp/state_published.txt

diff /tmp/events_published.txt /tmp/state_published.txt
```

They should match exactly. If they don't, events are authoritative. ([#93](https://github.com/EffortlessMetrics/shipper/issues/93) tracks an end-of-run consistency check that does this automatically.)

## See also

- [INVARIANTS.md](../INVARIANTS.md) — the truth/projection/summary contract
- [Tutorial: Recover from an interrupted release](../tutorials/recover-from-interruption.md)
