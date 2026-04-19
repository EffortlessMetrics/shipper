# Release Runbook — operator crib sheet

One-page operator procedure for cutting a crates.io release train via Shipper.

This is the "what do I actually type and when do I stop" doc. For the broader how-to (workflow YAML, Trusted Publishing setup), see [`how-to/run-in-github-actions.md`](./how-to/run-in-github-actions.md). For the last historical per-crate manifest (rc.1 tarball contents and topo proof), see [`release-v0.3.0-rc.1-manifest.md`](./release-v0.3.0-rc.1-manifest.md) — kept as an artifact of the first publish, not as the moving reference.

Shipper dogfoods its own release: `shipper plan → preflight → publish` drive the train end-to-end, with `shipper resume` for recovery. The workflow is tag-driven (`.github/workflows/release.yml`): pushing a `vX.Y.Z` tag triggers `publish-crates-io`.

## Crates in the train

Thirteen crates publish in this dependency order. Tier boundaries matter: crates-io's new-crate rate limit (5 burst, then 1 per 10 min) applies only to the first publish of each crate, so the first release train after adding a crate to the workspace is the one where wall-clock balloons.

Tier 1 — leaves: `shipper-cargo-failure`, `shipper-duration`, `shipper-encrypt`, `shipper-output-sanitizer`, `shipper-retry`, `shipper-sparse-index`, `shipper-webhook`.
Tier 2: `shipper-types` (depends on Tier 1 leaves).
Tier 3: `shipper-config`, `shipper-registry` (depend on `shipper-types`).
Tier 4: `shipper-core` (engine; depends on Tiers 1–3).
Tier 5: `shipper-cli` (adapter; depends on `shipper-core`).
Tier 6: `shipper` (install façade; depends on `shipper-cli`).

The authoritative order for a given release is whatever `shipper plan` prints for that commit — always trust the plan artifact, not this doc, if they disagree.

---

## Pre-flight (before cutting the tag)

1. **CI is green on `main`.** Every lane in the latest `CI` run for `main` must show success (`gh run list --workflow=ci.yml --branch=main --limit=1`). `architecture-guard` has a `paths:` trigger gate — if it hasn't re-posted a status since the last `crates/shipper/src/**` commit, verify the workflow file on `main` still has the `--include='*.rs'` filter (false-red guard from #85).
2. **Rehearsal is green.** `gh workflow run release.yml --ref main --field mode=rehearse` completed successfully. The plan ID in the uploaded `shipper-rehearse-<run_id>` artifact must match the plan ID from a local `shipper plan` on the same SHA.
3. **No mainline changes since the rehearsal.** Any commit to `main` after the rehearsal invalidates the plan ID. If mainline moved, re-rehearse.
4. **Version is bumped and committed.** `cargo metadata --format-version 1 | jq -r '.workspace_default_members[0]' | ...` → every publishable crate in `Cargo.toml` reads the intended `vX.Y.Z`. `CHANGELOG.md` has an entry for the new version (not `[Unreleased]`).
5. **crates.io is healthy.** Open [status.crates.io](https://status.crates.io/) immediately before starting. If the **git index** is running behind but the **sparse index** is healthy, that's OK — the workflow uses `--readiness-method both` and will use the sparse index path. If the **sparse index** itself is reporting incidents, abort and wait.
6. **Auth is present.**
   - **Token fallback (primary path today).** `CARGO_REGISTRY_TOKEN` repo secret must be set with publish scope for every crate in the plan. Trusted Publishing is wired in `release.yml` but not yet configured per-crate on crates.io — the OIDC step has `continue-on-error: true` and cleanly falls through to the token. Until Trusted Publishing is registered for every crate (see below), the token is what's actually doing the auth.
   - **Trusted Publishing (target path).** When you're ready to switch, follow the one-time-registration checklist in [`how-to/run-in-github-actions.md`](./how-to/run-in-github-actions.md#token-vs-trusted-publishing). Every crate in the plan must be registered as a trusted publisher for this repo + `release.yml` + `release` environment before the next tag push, or you'll mid-train 401 on the unregistered ones. Rehearse with the `release` environment bound (the workflow already does) to prove the scope wiring before going live.

## Cut the tag

Use the workspace version already committed to `Cargo.toml`. Read it once:

```bash
VERSION=$(cargo metadata --format-version 1 --no-deps \
  | jq -r '.packages[] | select(.name=="shipper") | .version')
echo "Releasing v$VERSION"
```

Then tag from `origin/main`, never from a local branch:

```bash
git fetch origin
git checkout origin/main
git tag -a "v$VERSION" -m "v$VERSION"
git push origin "v$VERSION"
```

Pushing the tag triggers `.github/workflows/release.yml` → `publish-crates-io` job.

## During the train

Expected wall-clock depends heavily on whether this is a first-publish of any crate or a re-publish of existing versions:

- **Re-publish of existing crates** (routine subsequent releases): typically well under 30 minutes; no new-crate rate limit, readiness polling is the dominant wait.
- **First-publish of new crates**: budget ~10 minutes per new crate past the initial 5-crate burst. A wave that adds multiple brand-new crates can stretch past an hour. The runner timeout in `release.yml` is 180 minutes for this reason.

### What to monitor

- **The workflow log** — watch for per-crate `shipper publish` events (`PackagePublishStarted`, `PackagePublished`, `PackageReadinessVerified`).
- **`.shipper/` artifact uploads.** Three are uploaded by the workflow: `shipper-state-plan`, `shipper-state-preflight`, `shipper-state-final`. The plan artifact uploads before any publish happens, so even a catastrophic runner death preserves the plan.
- **crates.io visibility.** After each publish, Shipper runs readiness checks (sparse index + API). You can also hit `https://index.crates.io/<prefix>/<crate>` directly for a fresh-resolver view of the sparse index.

### Stop conditions

| Situation | Action |
|---|---|
| `Permanent` error (auth, version conflict, manifest) | **Stop.** Fix the cause, bump version, re-tag. Never retry a permanent error. |
| `Retryable` error (429, transient network) | Let the engine retry — `--max-attempts 12`, `--max-delay 15m` is configured to ride out rate-limit windows. |
| `Ambiguous` error (upload may have succeeded) | **Let Shipper reconcile.** As of rc.2, the engine polls the registry on ambiguous and resolves to `Published` / `NotPublished` / `StillUnknown` without blind-retrying. `StillUnknown` halts for operator review — that's your stop signal, not a generic ambiguous. |
| Runner dies / 180-min timeout | `.shipper/` artifact is still uploaded. Use Resume below. |
| crates.io status page reports a new incident mid-train | Let the engine absorb 429s; only cancel the workflow if the incident is specifically hitting the sparse index. |
| Unexpected silence (no progress in >20 min, no events appended) | Check runner resource state. Don't cancel unless certain — a rate-limit sleep is expected between new-crate publishes. |

### Do NOT

- Run `cargo publish` manually on any crate in the plan mid-train. Trust the state file.
- Kill the workflow to "try again fresh" without first reading `.shipper/state.json` (or `events.jsonl`) to understand what completed.
- Merge any PR to `main` while the train is live — it invalidates the plan ID and prevents resume.

## Resume

If the train stopped and you need to pick up where it left off:

```bash
# Find the prior run's ID
gh run list --workflow=release.yml --limit=5

# Dispatch resume against that run's uploaded .shipper/ artifact
gh workflow run release.yml \
  --ref "v$VERSION" \
  --field mode=resume \
  --field artifact_run_id=<prior-run-id>
```

The `resume` path downloads the prior `shipper-state-final` artifact into `.shipper/` and runs `shipper resume`. Plan-ID validation aborts if the workspace has changed since the original run — don't try to "fix and resume." Cut a new RC instead.

## Post-train verification

Only finalize the GitHub Release after **every crate in the plan is visible on crates.io from a fresh resolver**:

1. Workflow log shows `shipper publish` completed successfully, all `PackageReadinessVerified` events emitted.
2. Every crate returns 200 from `https://crates.io/api/v1/crates/<crate>`.
3. `cargo search shipper` (no path override, clean cache) returns the new version. Repeat for `shipper-cli` and `shipper-core` — these are the three user-facing crate names.
4. At least one smoke install from a scratch directory:
   ```bash
   cargo install shipper --version "$VERSION" --locked
   shipper --version
   ```
   (`cargo install shipper-cli --version "$VERSION" --locked` also works — same code path, installs the adapter binary directly.)
5. The `shipper-state-final` artifact is downloaded and archived. GitHub retains it for 90 days; take a local copy for the permanent record — it's the events-as-truth evidence for that release.

Only then do the per-platform binary artifacts get attached to the GitHub Release. The release note should reference the `shipper-release-state.tar.gz` bundle as publish evidence.

## If you need to walk it back

Cargo's containment primitive is `cargo yank`. Yanking does NOT remove the published artifact; it only removes the version from future resolution. Existing `Cargo.lock` files continue to resolve yanked versions. Treat yank as containment, not undo.

As of rc.2, Shipper has receipt-driven containment via `shipper plan-yank` and `shipper yank` (see [#98](https://github.com/EffortlessMetrics/shipper/issues/98)). Prefer those over manual `cargo yank` — they read the receipt, generate a reverse-topological plan, and emit `PackageYanked` events to the ledger.

```bash
# Generate a containment plan from the release's receipt
shipper plan-yank --state-dir .shipper --output yank-plan.json

# Execute it
shipper yank --plan yank-plan.json
```

Manual fallback, if you don't have the receipt handy, is reverse topological order — install face first, then adapter, then core, then tiers 3 → 2 → 1. `cargo yank --vers "$VERSION" <crate>` per crate.

**Fix-forward** (bump the affected crate to the next patch/rc and re-release just that slice) is almost always preferable to a full yank cascade. `shipper fix-forward --mark-compromised <crate>@<version>` plans the minimal repair; the receipt schema records `compromised_at`, `compromised_by`, `superseded_by` so the history survives.
