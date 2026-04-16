# Release Runbook — operator crib sheet

One-page operator procedure for cutting a crates.io release train via Shipper.
Full technical detail lives in [`release-v0.3.0-rc.1-manifest.md`](./release-v0.3.0-rc.1-manifest.md) — this doc is the "what do I actually type and when do I stop."

Current RC baseline: `v0.3.0-rc.1`, commit [`5beab9b`](https://github.com/EffortlessMetrics/shipper/commit/5beab9b).

---

## Pre-flight (before cutting the tag)

1. **CI is green on `main`.** Every lane in the latest `CI` run for `main` must show success (`gh run list --workflow=ci.yml --branch=main --limit=1`). `architecture-guard` has a `paths:` trigger gate — if it hasn't re-posted a status since the last `crates/shipper/src/**` commit, verify the workflow file on `main` contains the `--include='*.rs'` filter (guard against false-reds predating #85).
2. **Rehearsal is green.** `gh workflow run release.yml --ref main --field mode=rehearse` completed successfully, plan ID in the uploaded `.shipper/` artifact matches the plan ID from local `shipper plan`.
3. **No mainline changes since the rehearsal.** Any commit to `main` after the rehearsal invalidates the plan ID. If mainline moved, re-rehearse.
4. **crates.io is healthy.** Open [status.crates.io](https://status.crates.io/) immediately before starting. If the **git index** is running behind but the **sparse index** is healthy, that's OK — the workflow is configured with `--readiness-method both` so it will hit the sparse index path. If the **sparse index** itself is reporting incidents, abort and wait.
5. **Token / auth is present.**
   - Trusted Publishing (preferred): the GitHub Actions OIDC path is configured for this repo on crates.io. Verify the trusted-publishing configuration for each of the 12 crates if they've been pre-registered.
   - Token fallback: `CARGO_REGISTRY_TOKEN` repo secret is set with publish scope for all 12 crates.

## Cut the tag

```bash
# from origin/main, never from a local branch
git fetch origin
git checkout origin/main
git tag -a v0.3.0-rc.1 -m "v0.3.0-rc.1 — first public crates.io wave"
git push origin v0.3.0-rc.1
```

Pushing the tag triggers `.github/workflows/release.yml` → `publish-crates-io` job.

## During the train

Expected wall-clock: **70–90 minutes.** Driven by the crates.io new-crate rate limit (5 burst, then 1 per 10 min). Topo order per the [manifest §"Topological publish order"](./release-v0.3.0-rc.1-manifest.md#topological-publish-order).

### What to monitor

- **The workflow log** — watch for the `shipper publish` per-crate events.
- **`.shipper/` artifact uploads** — there are three: `shipper-state-plan`, `shipper-state-preflight`, `shipper-state-final`. The plan artifact uploads before any publish happens, so even a catastrophic runner death preserves the plan.
- **crates.io visibility** — after each publish, `shipper` runs readiness checks (sparse index + API). You can also hit `https://index.crates.io/<prefix>/<crate>` directly for a fresh-resolver view (see the manifest for the URL pattern).

### Stop conditions

| Situation | Action |
|---|---|
| `Permanent` error (auth, version conflict, manifest) | **Stop.** Fix the cause, bump version, re-tag. Do NOT retry a permanent error. |
| `Retryable` error (429, transient network) | Let the engine retry — `--max-attempts 12`, `--max-delay 15m` is configured to ride out rate-limit windows. |
| `Ambiguous` error (upload may have succeeded) | **Pause.** Query the sparse index and API directly for `<crate>@0.3.0-rc.1`. Only resume after out-of-band confirmation; see Resume below. |
| Runner dies / 180-min timeout | `.shipper/` artifact is still uploaded. Use Resume below. |
| crates.io status page reports a new incident mid-train | Let the engine absorb 429s; only cancel the workflow if incidents are hitting the sparse index specifically. |
| Any unexpected silence (no progress in >20 min, no events appended) | Check the runner's resource state. Don't cancel unless certain — a rate-limit sleep is expected. |

### Do NOT

- Run `cargo publish` manually on any crate in the plan mid-train. Trust the state file.
- Kill the workflow to "try again fresh" without first reading `.shipper/state.json` to understand what completed.
- Merge any PR to `main` while the train is live — it invalidates the plan ID.

## Resume

If the train stopped and you need to pick up where it left off:

```bash
# Find the prior run's ID
gh run list --workflow=release.yml --limit=5

# Dispatch resume against that run's uploaded .shipper/ artifact
gh workflow run release.yml \
  --ref main \
  --field mode=resume \
  --field artifact_run_id=<prior-run-id>
```

The `resume` path downloads the prior `shipper-state-final` artifact into `.shipper/` and runs `shipper resume`. Plan-ID validation will abort if the workspace has changed since the original run — don't try to "fix and resume," cut a new RC instead.

## Post-train verification

Only finalize the GitHub Release after **all 12 crates are visible on crates.io** from a fresh resolver:

1. Workflow log shows `shipper publish` completed successfully.
2. Every crate returns a 200 from `https://crates.io/api/v1/crates/<crate>`.
3. `cargo search shipper-cli` (no path override) returns `0.3.0-rc.1`.
4. At least one smoke install: `cargo install shipper-cli --version 0.3.0-rc.1 --locked` from a scratch directory.
5. The `shipper-state-final` artifact is downloaded and archived (90-day retention by default; take a local copy for the permanent record).

Only then do the per-platform binary artifacts get attached to the GitHub Release. The release note should reference the `shipper-state-final` tarball as publish evidence.

## If you need to walk it back

Cargo's containment primitive is `cargo yank`. Yanking does NOT remove the published artifact; it only removes the version from future resolution. Existing `Cargo.lock` files continue to resolve yanked versions. Treat yank as containment, not undo.

Automated receipt-to-yank is a planned feature (remediation milestone, not yet implemented). Until then, yank manually in reverse topological order:

```bash
cargo yank --vers 0.3.0-rc.1 shipper-cli
cargo yank --vers 0.3.0-rc.1 shipper
cargo yank --vers 0.3.0-rc.1 shipper-config shipper-registry
cargo yank --vers 0.3.0-rc.1 shipper-types
cargo yank --vers 0.3.0-rc.1 shipper-cargo-failure shipper-duration shipper-encrypt shipper-output-sanitizer shipper-retry shipper-sparse-index shipper-webhook
```

Fix-forward (bump the affected crate to `0.3.0-rc.2` and re-release just that slice) is almost always preferable to a full yank cascade.
