# Tutorial: Recover from an interrupted release

In this tutorial you'll deliberately interrupt a Shipper publish run, inspect the persisted state, and use `shipper resume` to complete the release without duplicating work.

## What you'll learn

- What gets written to `.shipper/` during a running publish
- How `plan_id` guards resume correctness
- How `shipper resume` decides what to skip and what to continue
- How to tell the difference between "we have state, let's continue" and "plan changed, we shouldn't continue"

## What you'll need

- A workspace that's already been through the [first-publish tutorial](first-publish.md)
- A new patch version ready to publish across multiple crates (so there are >1 step to interrupt in the middle of)
- About 10 minutes

## 1. Start a publish

```bash
shipper publish
```

Watch the output. After the first crate is published (you'll see a line like `published shipper-duration@0.0.2`), open a second terminal:

## 2. Interrupt it

Kill the process. On Unix:

```bash
# in a second terminal
pkill shipper
```

Or press `Ctrl+C` in the first terminal. Shipper's signal handler writes final state before exiting.

## 3. Inspect the persisted state

```bash
ls -la .shipper/
```

You'll see:

- `events.jsonl` — append-only event log (grew during the run, stopped when you killed it)
- `state.json` — projection: what's published so far, what's pending
- `lock` — the concurrent-publish guard. Released cleanly on signal; may still be present on SIGKILL.

```bash
shipper inspect-events | tail -20
```

The last events should show package_started / package_attempted / package_published for the crate that completed, then nothing further. No `execution_finished`.

## 4. Resume

```bash
shipper resume
```

Shipper will:

1. Load `.shipper/state.json`
2. Recompute the current plan from your workspace
3. **Verify the plan_id matches**. If the workspace changed between runs, Shipper refuses to continue (this is the safety guardrail that prevents partial-publish corruption).
4. Skip packages already marked published
5. Continue from the first pending package

You should see output like `skipping shipper-duration (already published)` followed by the next crate in the queue.

## 5. What if the plan changed?

Deliberately simulate: edit `Cargo.toml` of one of your crates (bump its version, add a dep, anything), then:

```bash
shipper resume
```

Shipper will refuse:

```
error: plan_id mismatch. The workspace has changed since the interrupted run.
  expected: 23ff8f85...
  got:      a11b7c42...
Use --force-resume to override (advanced: may cause duplicate publish attempts).
```

Revert the change, then `shipper resume` works again.

## 6. Understanding the truth model

The key invariant is: **events are truth, state is a projection**.

If `state.json` ever disagrees with `events.jsonl`, events win. Full details: [explanation/INVARIANTS.md](../INVARIANTS.md).

In practice this means:

- `state.json` can be rebuilt from `events.jsonl` if it's corrupted
- `resume` reads `state.json` for speed, but the ground truth is in events
- Any tool you build on top of Shipper should prefer events for correctness, state for speed

## 7. What's next

- Running this flow in CI requires a bit of care: the `.shipper/` directory must be uploaded as an artifact so a re-run can download it. See [how-to/run-in-github-actions.md](../how-to/run-in-github-actions.md).
- To understand why resume exists and what edge cases still need work, see issues [#90](https://github.com/EffortlessMetrics/shipper/issues/90) and [#101](https://github.com/EffortlessMetrics/shipper/issues/101).
