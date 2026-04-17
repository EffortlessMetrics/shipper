# Understanding `finishability` — especially `not_proven`

`shipper preflight` ends by emitting a `finishability` value — one of three states. Two of them are obvious; the third reads alarming but is usually fine. This page explains what each means, why `not_proven` is the *correct* answer in common cases, and when to worry vs when to proceed.

## The three states

```rust
Finishability::Proven     // every check came back positive; publish is expected to succeed
Finishability::NotProven  // checks ran without errors, but some couldn't complete to "proven"
Finishability::Failed     // preflight found something actually wrong; do not publish
```

Preflight runs these checks, roughly in order:
1. **Git cleanliness** — working tree has no uncommitted changes (unless `--allow-dirty`)
2. **Registry reachability** — the API base URL responds
3. **Workspace dry-run** — `cargo publish --dry-run` succeeds across the workspace
4. **Version-not-taken** — for each crate, the version about to be published isn't already on the registry
5. **Ownership** (best-effort) — the configured token is listed as an owner of each crate

`Failed` triggers on hard violations: dirty tree without `--allow-dirty`, registry unreachable, dry-run fails, version already taken. Something is actually broken; don't publish.

`Proven` triggers when everything — including ownership — comes back positive.

`NotProven` triggers when the *completed* checks all passed but at least one check *couldn't reach a conclusion*. This is the state that reads scary and usually isn't.

## Why `not_proven` is the correct answer for first-publish runs

The canonical case: you're publishing a brand-new crate to crates.io for the first time.

- Version-not-taken? ✅ Passes — the crate doesn't exist yet, so no version is taken.
- Workspace dry-run? ✅ Passes — cargo can package the crate locally.
- **Ownership?** ❓ Can't complete. There's no crate yet, so there's no owners endpoint to query. The check returns "not verified" — not because you *aren't* the owner, but because the question doesn't apply yet.

Preflight correctly refuses to claim `Proven` in this case — it would be lying. The crate doesn't exist yet; ownership is literally unverifiable until after the first publish. But nothing is *wrong*; the dry-run passed, the registry is reachable, the version is available. So the honest answer is `NotProven`.

On the v0.3.0-rc.1 first publish of 12 new Shipper crates, preflight correctly reported `finishability=not_proven` for every one, and the publish train completed successfully.

## What to do when you see `not_proven`

### If this is a first publish of new crates
Proceed. The ownership check can't complete until the crate exists; you have no way to advance past `not_proven` for a brand-new crate except by publishing it. The release.yml workflow in this repo deliberately treats `not_proven` as non-failing because of exactly this case.

### If this is a new version of an existing crate and you see `not_proven`
Read the preflight report carefully — it lists the specific checks that didn't conclude. Common causes:

- **Token doesn't have owner permissions** on the crate — real fix: `cargo owner --add` or rotate the token.
- **Registry API temporarily returned an unexpected shape** — usually a crates.io hiccup; re-run preflight.
- **Network flake on the owners endpoint** — re-run.

### If you're not sure
Run `shipper preflight --format json` and inspect the per-package detail. Each `PreflightPackage` entry shows which individual checks passed or didn't conclude, so you can tell whether it's the "new crate, owners endpoint doesn't apply" case or something genuinely actionable.

## Why Shipper doesn't auto-promote `not_proven` to `proven`

We considered it — rename `not_proven` to `provisional_new_crate` when the only unverified check is ownership on a crate that doesn't exist yet — but that's a product-wording change that can happen anytime. The *state* is correct.

A future upgrade path exists via the rehearsal registry ([#97](https://github.com/EffortlessMetrics/shipper/issues/97)): publishing the exact packaged artifacts to an alternate registry first, then resolving + installing from that registry, would let preflight promote many `NotProven` cases to `Proven`. That's the next tier of proof; until it lands, first-publish `NotProven` is epistemically honest and operationally fine.

## The short version

| Situation | `finishability` | Action |
|---|---|---|
| Everything green | `Proven` | Publish |
| First publish of new crate; ownership can't be verified yet | `NotProven` | **Publish** — this is normal |
| Token lacks ownership on an existing crate | `NotProven` | Fix ownership (`cargo owner --add`) or rotate token |
| Git dirty, dry-run failed, version taken, registry unreachable | `Failed` | Do not publish; fix the actual problem |

## See also

- [MISSION.md — the single test](../../MISSION.md) (`you can start a release train...`)
- [why-shipper.md](why-shipper.md#why-finishability-has-three-states) — the deeper "why three states"
- [#97 Rehearsal registry](https://github.com/EffortlessMetrics/shipper/issues/97) — future path to promote more `NotProven` cases to `Proven`
- [preflight.md](../preflight.md) — what each preflight check does
