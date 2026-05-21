# Shipper Swarm Sync

Status: active

`EffortlessMetrics/shipper` remains the release authority for Shipper:
crates.io publishing, release evidence, tags, GitHub Releases, release
workflow credentials, and signing credentials stay here until release authority
is deliberately moved.

Routine development happens in
[`EffortlessMetrics/shipper-swarm`](https://github.com/EffortlessMetrics/shipper-swarm).

## Repository Roles

| Repository | Role | Normal merge method |
|---|---|---|
| `EffortlessMetrics/shipper-swarm` | Active development | Squash merge PRs |
| `EffortlessMetrics/shipper` | Release authority and provenance | Merge commits |

## What Belongs Here

Use `EffortlessMetrics/shipper` for:

- swarm sync PRs
- release-authority docs
- crates.io and GitHub Release workflow changes
- release evidence and readiness proof updates
- signing, provenance, or Trusted Publishing changes
- emergency hotfixes, when explicitly declared

Use `EffortlessMetrics/shipper-swarm` for routine feature work, refactors,
tests, and normal development PRs.

## Sync Policy

Syncs from `shipper-swarm/main` back to `shipper/main` preserve swarm commit
history. Do not squash or rebase sync PRs.

Create sync branches from `shipper/main` and merge `shipper-swarm/main` with
a merge commit:

```bash
git remote add swarm git@github.com:EffortlessMetrics/shipper-swarm.git
git fetch origin --prune --tags
git fetch swarm --prune

git switch -c sync/shipper-swarm-YYYY-MM-DD origin/main
git merge --no-ff swarm/main -m "merge: sync shipper-swarm development"
git push -u origin sync/shipper-swarm-YYYY-MM-DD
```

Open the PR in `EffortlessMetrics/shipper` and merge it with a merge commit.
Do not use squash merge or rebase merge.

After a swarm-sync PR lands, fast-forward `shipper-swarm/main` to the new
`shipper/main` merge commit before continuing normal swarm development:

```bash
git fetch origin --prune --tags
git fetch swarm --prune

git merge-base --is-ancestor swarm/main origin/main
git push swarm origin/main:main
```

If the merge-base check fails, stop the fast-forward path and use the
source-backfill path below. Do not force push, squash, or rebase the sync
commit.

If the source repo receives a release-authority PR that was not a swarm sync
while `shipper-swarm` already has unsynced development commits, do not
fast-forward swarm to source. Open a source-backfill PR in `shipper-swarm` that
merges `shipper/main` into `shipper-swarm/main`, and merge that backfill with a
merge commit. This preserves both the source merge commit and the swarm
development commits.

## Credential Boundary

Do not move these into `shipper-swarm` without a separate release-authority
migration plan:

- `CARGO_REGISTRY_TOKEN`
- crates.io publish tokens
- release signing secrets
- GitHub Release publish credentials
- Trusted Publishing release authority
