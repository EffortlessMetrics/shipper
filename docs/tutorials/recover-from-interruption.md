# Tutorial: Recover from an interrupted release safely

This tutorial exercises Shipper's interruption and resume journey with fake
Cargo, a mock registry, and a real cross-job artifact handoff. It is the
default rehearsal because it proves durable recovery mechanics without
creating public registry versions.

> **Do not learn recovery by cancelling a real crates.io publish.** A real
> upload may succeed before the client reports failure, and yanking is
> containment rather than deletion. Destructive real-registry rehearsal is a
> separate release-authority operation.

## What you'll learn

- Which `.shipper/` files are retained across an interruption.
- Why events are authoritative and state is a projection.
- How `status --durable` fails closed on disagreement or uncertain liveness.
- How the deterministic `plan_id` guards resume, and what it does not bind.
- Why `StillUnknown` never supplies a blind retry command.

## 1. Run the safe rehearsal

Anyone working from this source checkout can run the local fake-Cargo and mock
registry fixture:

```bash
cargo test -p shipper-cli --test e2e_rehearse \
  rehearsal_interrupted_publish_then_resume_preserves_invariants \
  -- --exact --nocapture
```

This proves the local interruption/resume invariants without registry writes.
The remaining steps inspect the cross-job version. They require Actions write
permission on the release-authority repository; users without that permission
can stop after the local fixture or use the
[alternate-registry rehearsal](../how-to/rehearse-against-an-alt-registry.md).

Maintainers can dispatch the active workflow:

```bash
gh workflow run live-runner-interruption-rehearsal.yml \
  --repo EffortlessMetrics/shipper \
  --ref main
```

Record the run ID, then wait for it to finish:

```bash
gh run watch <run-id> --repo EffortlessMetrics/shipper
```

The workflow uses one job to create interrupted evidence and a separate job to
download it and resume. It never publishes to crates.io.

## 2. Download both evidence artifacts

```bash
gh run download <run-id> \
  --repo EffortlessMetrics/shipper \
  --name shipper-live-interruption-seed-<run-id> \
  --dir seed-evidence

gh run download <run-id> \
  --repo EffortlessMetrics/shipper \
  --name shipper-live-interruption-resume-<run-id> \
  --dir resumed-evidence
```

Keep the seed and resumed directories separate. The seed should contain
`events.jsonl`, `state.json`, and the fake-Cargo command log. The completed
resume artifact should additionally contain `receipt.json`.

## 3. Inspect the interrupted evidence

The workflow artifacts contain `.shipper/` evidence, not their matching source
workspace. Inspect them as raw evidence rather than invoking a command that
must recompute the release plan:

```bash
jq -c '.' seed-evidence/events.jsonl | tail -20
jq -r '.plan_id, (.packages[] | "\(.name)@\(.version): \(.state.state)")' \
  seed-evidence/state.json
```

Do not run `status --durable` from an artifact-only directory. It recomputes a
release plan and needs the exact matching workspace and candidate binary.

## 4. Inspect the completed resume

Inspect the completed artifact without combining it with the seed:

```bash
jq -c '.' resumed-evidence/events.jsonl | tail -20
jq '.' resumed-evidence/state.json
jq '.' resumed-evidence/receipt.json
```

Verify that:

- already-published packages were skipped rather than republished;
- the same `plan_id` and workspace/run identity bind both segments;
- `events.jsonl` contains one authoritative publish outcome per package;
- `state.json` agrees as the resumable projection;
- `receipt.json` summarizes the terminal run.

## 5. Apply the rule to a real interruption

After an actual runner cancellation, restore the entire `.shipper/` directory
inside the exact source checkout and use the matching candidate binary. Start
from that workspace root with:

```bash
shipper status --durable
shipper inspect-events
```

Run `shipper resume` only when the durable result and retained evidence say the
run is interrupted and safe to resume. Resume recomputes the current plan and
refuses when its `plan_id` differs from the stored value. That ID covers the
registry API base and ordered package names/versions; it does not bind source
bytes, workspace path, or run identity. Restore and independently verify the
exact checkout rather than treating the plan-ID check as source provenance.

The durable result is intentionally fail-closed. `no_evidence` means no durable
run was found; `identity_mismatch` means the current plan ID, registry, or
retained evidence identity differs; `evidence_disagreement` means the retained
sources contradict each other; and `unknown` means an unfinished run's
liveness cannot be established. None authorizes resume. On Linux, `Live`
requires an exact local boot, PID namespace, PID, and process-start match.
`NotLive` means the process was absent inside the same proven scope; it is not
by itself permission to resume.

Do not use `--force-resume` as a normal recovery step. It overrides a plan
guard and requires a separate operator risk decision.

## 6. Ambiguity is not interruption

If Cargo's result is ambiguous, Shipper queries registry truth before any new
publish attempt:

- `Published`: retain the reconciliation evidence and advance.
- `NotPublished`: the normal policy may retry.
- `StillUnknown`: stop, retain `reconciliation.json`, and investigate. No
  generated retry command is safe.

Use `shipper inspect-events`, `shipper inspect-receipt` when a receipt exists,
and the paths reported by `status --durable`. Never infer safety from a quiet
log, an old lock timestamp, or exit code alone.

## See also

- [Run the Recover rehearsal](../how-to/run-recover-rehearsal.md)
- [Inspect a stalled run](../how-to/inspect-a-stalled-run.md)
- [Inspect state, events, and receipts](../how-to/inspect-state-and-receipts.md)
- [INVARIANTS.md](../INVARIANTS.md)
