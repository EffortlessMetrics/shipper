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

Immediately after a true swarm-sync PR lands in `shipper`, `shipper/main`
contains a merge commit that may not yet exist in `shipper-swarm/main`. During
that short window the count can be `1 0`. Fast-forward `shipper-swarm/main` to
`shipper/main` before reopening normal swarm development.

If a release-authority PR lands directly in `shipper` while `shipper-swarm` is
already ahead, do not fast-forward swarm to source because that would discard
swarm development commits. Merge `shipper/main` back into `shipper-swarm/main`
with the source-backfill process below.

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

After a swarm-sync PR lands, backfill the release-authority merge commit into
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
check returns `0 N` again. If the merge-base command fails, use the
source-backfill path instead; do not force push or squash the sync commit.

## Source-Backfill Exception

Release-authority PRs may land directly in `shipper` for release evidence,
credentials, signing, or repository policy. When that happens while
`shipper-swarm` has unsynced development commits, backfill the source commit
into swarm with a merge commit:

```bash
git fetch origin --prune
git fetch public --prune --tags

git switch -c backfill/shipper-source-YYYY-MM-DD origin/main
git merge --no-ff public/main -m "merge: backfill shipper release-authority changes"
git push -u origin backfill/shipper-source-YYYY-MM-DD
```

Open the backfill PR in `EffortlessMetrics/shipper-swarm`. This is not a normal
development PR: merge it with a merge commit so `shipper/main` becomes an
ancestor of `shipper-swarm/main` again. Temporarily allowing merge commits for
that PR is acceptable; restore squash-only settings immediately afterward.

## CI and Branch Protection

`shipper-swarm/main` requires only the normalized result check:

```text
Shipper Rust Small Result
```

Do not require route-specific implementation jobs directly. Only one of those
jobs is expected to run on each attempt.

Current routed Rust-small proof:

- CPX42-first routing passed on PR #31 with `Routed Rust Small` run
  `26244152934`.
- Normal same-repo CPX42 routing passed again on PR #22 with run
  `26252949412` and PR #17 with run `26256205458`.
- GitHub-hosted fallback execution passed on PR #24 with run `26247605774`.

## Credential Boundary

Do not add these to `shipper-swarm`:

- `CARGO_REGISTRY_TOKEN`
- crates.io publish tokens
- release signing secrets
- GitHub Release publish credentials

Release credentials stay in `EffortlessMetrics/shipper` until release
authority is deliberately migrated.

The release workflow may exist in `shipper-swarm` because the two repositories
share history and sync commits. Any release-authority workflow job that can
publish crates, mint release tokens, create GitHub Releases, or upload release
artifacts must be guarded with:

```text
github.repository == 'EffortlessMetrics/shipper'
```

That guard keeps tag and manual workflow runs inert in `shipper-swarm` while
preserving the same workflow file for merge-commit syncs back to the release
authority repository.
