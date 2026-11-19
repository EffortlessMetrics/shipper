# How to inspect a stalled or interrupted run

Use this path when a workflow is quiet, a runner was cancelled, or you need to
decide whether retained evidence supports resume. Do not infer safety from log
silence, a lock's age, or `state.json` alone.

## Triage: which surface answers which question?

| Question | Authority | Command |
|---|---|---|
| What can local durable evidence prove? | correlated events/state/receipt/lock evidence | `shipper status --durable` |
| What happened, and in what order? | `events.jsonl` | `shipper inspect-events` |
| What was the last durable event? | `events.jsonl` | `shipper inspect-events` or raw `jq` |
| Which packages are projected complete? | `state.json` projection | inspect `packages[].state.state` |
| What did a terminal run summarize? | `receipt.json` | `shipper inspect-receipt` |
| Why did ambiguity stop? | events plus `reconciliation.json` | inspect the retained paths |

Events are authoritative, state is a resumable projection, and a receipt is a
derived terminal summary. When they disagree, stop and investigate.

## 1. Start with the fail-closed durable view

```bash
shipper status --durable
```

This local mode bypasses registry access. It correlates the active event
segment with plan, workspace, state, receipt, reconciliation, and lock
identity. Its fail-closed status distinguishes `no_evidence`,
`identity_mismatch`, `evidence_disagreement`, and `unknown`: respectively no
durable run, mismatched source/plan/registry/evidence identity, contradictory
evidence, or an unfinished run whose liveness cannot be established. None of
those statuses authorizes resume.

On Linux, `Live` requires an exact local boot, PID namespace, PID, and
process-start match. `NotLive` means the matching-scope process was observed
absent; it is not by itself a safe-resume verdict. Follow the reported rerun
posture and reason.

## 2. Read the latest authoritative events

```bash
shipper inspect-events

# Raw tail when jq is available
jq -c '.' .shipper/events.jsonl | tail -20
```

A `retry_backoff_started` event records why the run waited, its attempt, and
`next_attempt_at`:

```bash
jq -c 'select(.event_type.type == "retry_backoff_started") | .event_type' \
  .shipper/events.jsonl | tail -1
```

A future timestamp explains the recorded wait; it does not independently
prove that the process is alive. Registry limits and visibility delays can
change, so prefer emitted evidence over hard-coded timing assumptions.

Find the latest package and authoritative publish outcomes with:

```bash
jq -r 'select(.event_type.type == "package_started") | .package' \
  .shipper/events.jsonl | tail -1

jq -r 'select(.event_type.type == "package_published") | .package' \
  .shipper/events.jsonl | sort -u
```

## 3. Compare the state projection

The package-state path is `packages[].state.state`, not
`packages[].status`:

```bash
jq -r '.packages[] | "\(.name)@\(.version): \(.state.state)"' \
  .shipper/state.json
```

A disagreement between this projection and `events.jsonl` is evidence drift,
not a reason to edit state or force resume.

## 4. Inspect failure and reconciliation evidence

```bash
jq -c 'select(.event_type.type == "package_failed") | .event_type' \
  .shipper/events.jsonl | tail -1

jq -c 'select(.event_type.type == "publish_reconciled") | .event_type' \
  .shipper/events.jsonl

jq '.' .shipper/reconciliation.json
```

`reconciliation.json` is conditional. When an ambiguous Cargo result becomes
`StillUnknown`, preserve it and investigate. Shipper deliberately supplies no
retry command while registry truth is inconclusive.

## 5. Decide whether resume is allowed

Run `shipper resume` only when `status --durable` reports agreeing retained
evidence and a safe rerun posture. Resume recomputes the plan and refuses a
stored `plan_id` mismatch before it skips terminal packages or reconciles
ambiguous outcomes. The plan ID covers the registry API base and ordered
package names/versions; it does not bind source bytes, workspace path, or run
identity.

On Linux and macOS, an explicit absolute `SHIPPER_CARGO_BIN` path that
is missing or inaccessible at the pre-spawn check can produce a
`package_not_launched` event. A failed bare `cargo` lookup on `PATH` or an
error returned by spawn does not produce that proof; an unresolved attempt
still blocks retry. If Cargo is missing during CLI planning, the command
normally stops before any publish attempt. Replacing Cargo cannot make an
older ambiguous attempt safe to retry; keep its evidence and investigate.

Do not use `--force-resume` as routine recovery. It overrides a plan guard and
requires a separate operator risk decision.

## CI interruption

Download the complete `.shipper/` artifact and restore it beside the exact
source checkout that produced it. Keep `events.jsonl`, `state.json`, optional
`reconciliation.json`, the lock record, and any terminal `receipt.json`
together. See [Run a Shipper release in GitHub Actions](run-in-github-actions.md)
for artifact handling and the
[safe recovery tutorial](../tutorials/recover-from-interruption.md) for a mock
cross-job rehearsal.

## See also

- [Inspect state, events, and receipts](inspect-state-and-receipts.md)
- [State files reference](../reference/state-files.md)
- [INVARIANTS.md](../INVARIANTS.md)
