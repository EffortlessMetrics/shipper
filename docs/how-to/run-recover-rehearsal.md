# How-to: Run the Recover rehearsal

Goal: prove, end-to-end against real crates.io, that Shipper's `resume`
path actually works under a real workflow interruption. This is the
operator side of [#90](https://github.com/EffortlessMetrics/shipper/issues/90).

Synthetic coverage lives in
`crates/shipper-cli/tests/e2e_rehearse.rs`, which exercises the same
invariants (state/events/skip/idempotency) against a mock registry and
fake cargo. That test runs on every CI commit; the procedure here runs
**once per release-candidate line** and is what catches bugs the mock
can't reach (crates.io rate-limit behavior, sparse-index propagation,
artifact upload on killed runners, etc.).

## Prerequisites

- Admin access to <https://github.com/EffortlessMetrics/shipper>.
- `CARGO_REGISTRY_TOKEN` with publish scope for all 12 crates already
  configured (or Trusted Publishing registration complete — see
  [run-in-github-actions.md](run-in-github-actions.md)).
- A throwaway version suffix that has not been used. Convention:
  `v0.3.0-test-resume-<YYYYMMDD>`.

## The rehearsal

### Step 1 — prep the throwaway tag

On a clean `origin/main`:

```bash
git fetch origin
git checkout origin/main
git tag -a v0.3.0-test-resume-$(date +%Y%m%d) -m "recover rehearsal"
git push origin v0.3.0-test-resume-$(date +%Y%m%d)
```

> **Do NOT** use a real RC version. Yanking is containment, not undo —
> a rehearsal tag pollutes crates.io if left unyanked.

### Step 2 — kick off the release workflow

Pushing the tag triggers `.github/workflows/release.yml` →
`publish-crates-io` job automatically.

Alternatively, dispatch manually:

```bash
gh workflow run release.yml --ref <tag>
```

### Step 3 — watch for the mid-run kill point

```bash
gh run watch --repo EffortlessMetrics/shipper
```

Once 2–3 crates show as published in the logs (the per-crate
`PackagePublished` event lines inside `shipper publish` stderr):

```bash
gh run cancel <run-id> --repo EffortlessMetrics/shipper
```

The `shipper-state-preflight` and `shipper-state-plan` artifacts will
already be uploaded. The `shipper-state-final` artifact is uploaded
`if: always()` so the cancellation itself triggers it — wait ~30s after
cancelling for the artifact upload to complete.

### Step 4 — collect evidence

```bash
gh run download <run-id> --repo EffortlessMetrics/shipper
```

Expect four directories:

- `shipper-state-plan/` — plan stage artifact
- `shipper-state-preflight/` — post-preflight artifact
- `shipper-state-final/` — the crucial one. Contains `state.json`,
  `events.jsonl`, `receipt.json` at the moment of cancellation.

### Step 5 — sanity-check the artifacts

Pull up `shipper-state-final/`:

```bash
cd shipper-state-final
# state.json parses; events.jsonl is valid NDJSON
shipper inspect events      # or: jq -c . events.jsonl | head
shipper inspect receipt
```

Expected shape:

- `state.json` has `state_version: "shipper.state.v1"` and a non-empty
  `plan_id`.
- Some packages have `state: "published"`; at least one is still
  `state: "pending"` (or `"uploaded"` / `"failed"`).
- `events.jsonl` ends with a complete line (no half-written event).
- `PackagePublished` event count equals the count of `published`
  packages in state.json — events-as-truth.

### Step 6 — trigger the resume

```bash
gh workflow run release.yml \
  --repo EffortlessMetrics/shipper \
  --ref <same-tag> \
  --field mode=resume \
  --field artifact_run_id=<run-id-from-step-3>
```

### Step 7 — verify the resume

The resume run should:

- Download `shipper-state-final` from the cancelled run into `.shipper/`.
- Validate `plan_id` matches current workspace plan_id (it should, same
  tag).
- Log `already published (skipping)` for each crate that was Published
  in the downloaded state.
- Run `cargo publish` **only** for the remaining packages.
- Produce a final `shipper-state-final` artifact with all 12 packages
  showing `state: "published"`.

Download the resume's artifact and spot-check:

```bash
gh run download <resume-run-id> --repo EffortlessMetrics/shipper
jq '.packages | to_entries | map({name: .key, state: .value.state.state})' \
    shipper-state-resume-*/state.json
```

Every entry should be `{"state": "published"}`.

### Step 8 — spot-check crates.io

```bash
for c in shipper shipper-cli shipper-core shipper-config shipper-types \
         shipper-duration shipper-retry shipper-encrypt shipper-webhook \
         shipper-registry shipper-sparse-index shipper-cargo-failure \
         shipper-output-sanitizer; do
  echo "- $c:"
  cargo search --limit 1 "$c" | head -1
done
```

Each line should show the rehearsal version.

### Step 9 — yank the rehearsal

```bash
for c in shipper shipper-cli shipper-core shipper-config shipper-types \
         shipper-duration shipper-retry shipper-encrypt shipper-webhook \
         shipper-registry shipper-sparse-index shipper-cargo-failure \
         shipper-output-sanitizer; do
  cargo yank --version <rehearsal-version> "$c"
done
```

Yanking is containment, not deletion — the bytes remain on crates.io,
but new resolves skip them.

> Once `shipper yank` / `shipper plan-yank` land (#98 PR2+),
> this step becomes a single `shipper plan-yank --from-receipt <file>`
> invocation against the rehearsal's `receipt.json`.

## Pass / fail rubric

The rehearsal passes iff **all** of the following are true:

- [ ] `shipper-state-final` artifact exists after the cancelled run.
- [ ] `state.json` parses; `plan_id` non-empty.
- [ ] `events.jsonl` is valid NDJSON (every line parses).
- [ ] `PackagePublished` event count = published-package count in
      state.json (events-as-truth).
- [ ] Resume does not re-`cargo publish` any crate that was already
      Published (check logs for duplicate `Publishing X@...` lines).
- [ ] Final state has every package Published.
- [ ] All crates visible on crates.io from a fresh resolver.

Any `[ ]` → file a bug citing the specific artifact. Don't ship the
release line the rehearsal was cut from until the regression is fixed
**and** the rehearsal has been re-run green.

## When to re-run

- Before cutting **every** new release-candidate minor (`v0.3.0-rc.N`,
  `v0.4.0-rc.N`, ...).
- After any change to `crates/shipper-core/src/engine/`,
  `crates/shipper-core/src/state/`, or `crates/shipper-core/src/runtime/`
  that touches persistence, resumption, or reconciliation.
- Whenever `.github/workflows/release.yml` changes the dispatch /
  resume shape.

The synthetic test at `crates/shipper-cli/tests/e2e_rehearse.rs` runs
on every CI commit and acts as a cheap pre-flight, but it's not a
substitute for this procedure — the real kill happens on a real runner
with a real registry.

## See also

- [Inspect a stalled run](inspect-a-stalled-run.md) — what to look for
  inside `.shipper/` without a rehearsal.
- [Release runbook](../release-runbook.md) — the production release
  procedure this rehearsal validates.
- [#90](https://github.com/EffortlessMetrics/shipper/issues/90) — the
  issue this procedure closes.
