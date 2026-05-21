# Shipper Swarm Operation

Status: active

`EffortlessMetrics/shipper-swarm` is the active development repository.
`EffortlessMetrics/shipper` remains the release authority for crates.io
publishing, release evidence, tags, and signing credentials until that
authority is explicitly moved.

## Repository Roles

| Repository | Role | Normal merge method |
|---|---|---|
| `EffortlessMetrics/shipper-swarm` | Active development | Squash merge PRs |
| `EffortlessMetrics/shipper` | Release authority and provenance | Merge commits |

## History Model

`shipper-swarm/main` must remain a continuation of `shipper/main` history:

```text
shipper/main:
A---B---C---D

shipper-swarm/main:
A---B---C---D---S1---S2---S3
```

It must not be seeded as an orphan snapshot. Snapshot seeding copies the tree
but loses ancestry, which breaks ahead/behind comparisons and makes later
promotion back to `shipper` ambiguous.

The expected steady-state ancestry check is:

```bash
git merge-base --is-ancestor public/main origin/main
git rev-list --left-right --count public/main...origin/main
```

The healthy count before a source sync, and again after source sync backfill,
is:

```text
0 N
```

where `N` is the number of swarm-only commits not yet synced back to
`shipper`.

Immediately after a sync PR lands in `shipper`, `shipper/main` contains a
merge commit that may not yet exist in `shipper-swarm/main`. During that short
window the count can be `1 0`. Fast-forward `shipper-swarm/main` to
`shipper/main` before reopening normal swarm development.

## Merge Policy

PRs into `shipper-swarm/main` are squash-merged. That keeps normal development
to one reviewed commit per PR.

Syncs from `shipper-swarm/main` back to `shipper/main` are never squashed or
rebased. Use a merge commit so the release authority preserves the sequence of
swarm-delivered commits:

```bash
git remote add swarm git@github.com:EffortlessMetrics/shipper-swarm.git
git fetch origin --prune --tags
git fetch swarm --prune

git switch -c sync/shipper-swarm-YYYY-MM-DD origin/main
git merge --no-ff swarm/main -m "merge: sync shipper-swarm development"
git push -u origin sync/shipper-swarm-YYYY-MM-DD
```

Open the sync PR in `EffortlessMetrics/shipper` and merge it with a merge
commit.

After the sync PR lands, backfill the release-authority merge commit into
`shipper-swarm` with a fast-forward update from the `shipper` checkout:

```bash
git fetch origin --prune --tags
git fetch swarm --prune

git merge-base --is-ancestor swarm/main origin/main
git push swarm origin/main:main
```

This update is not a normal swarm development PR. It preserves the source
merge commit so the next swarm development commit again starts from
`shipper/main` ancestry. Keep normal swarm PR merges paused until the ancestry
check returns `0 N` again. If the merge-base command fails, stop; do not force
push or squash the sync commit.

## CI and Branch Protection

`shipper-swarm/main` requires only the normalized result check:

```text
Shipper Rust Small Result
```

Do not require route-specific implementation jobs directly. Only one of those
jobs is expected to run on each attempt.

## Credential Boundary

Do not add these to `shipper-swarm`:

- `CARGO_REGISTRY_TOKEN`
- crates.io publish tokens
- release signing secrets
- GitHub Release publish credentials

Release credentials stay in `EffortlessMetrics/shipper` until release
authority is deliberately migrated.
