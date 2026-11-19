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

## Release Freeze and Promotion Evidence

A release candidate is an exact merged `shipper-swarm/main` commit, not a PR
head, merge ref, branch name, or collection of green checks from different
SHAs. Use the [release preparation checklist](../release/release-preparation-checklist.md)
to record at least:

- frozen swarm candidate SHA and tree;
- exact candidate-head Rust, policy, package, compatibility, and Changie proof;
- promotion PR, non-fast-forward merge SHA, and merge tree;
- release-authority approved SHA and tree after any separately reviewed
  release-only change;
- rehearsal, binary, interruption/resume, tag, publish, and final artifact
  identities;
- final release-authority-to-swarm backfill SHA.

When a candidate is frozen:

1. pause normal swarm merges;
2. prove the merged candidate, including `cargo changelog-roundtrip` locally
   with the pinned Changie binary;
3. record `git rev-parse HEAD` and `git rev-parse HEAD^{tree}`;
4. promote with `git merge --no-ff "$SWARM_SHA"` from current `shipper/main`,
   after verifying the fetched `swarm/main` ref still equals the recorded
   frozen SHA and tree;
5. require the promotion merge tree to equal the frozen swarm tree before any
   separately reviewed release-authority-only change;
6. merge the promotion PR with a merge commit;
7. keep normal swarm development paused until the resulting source merge and
   any final release-only commits are backfilled.

The tree-equality check is blocking:

```bash
test "$(git rev-parse swarm/main)" = "$SWARM_SHA"
test "$(git rev-parse "$SWARM_SHA^{tree}")" = "$SWARM_TREE"
test "$(git rev-parse HEAD^{tree})" = "$SWARM_TREE"
```

A conflict, manual content edit, or tree mismatch is not ordinary release
preparation. Stop, restore the source/swarm ancestry relationship through a
reviewed backfill, rebuild the swarm candidate, and promote again.

Any commit after the frozen swarm SHA supersedes that candidate. Historical
semver, binary, compatibility, or security proof may support an unchanged claim
only when the new candidate record names both SHAs, records the complete diff
boundary, and explains why the diff cannot affect the claim. The exact-head
Rust gate and release-authority rehearsal are always rerun.

A green swarm candidate is not publication authorization. Tags, rehearsal,
resume, crates.io publication, GitHub Release creation, signing, and release
credentials remain release-authority operations in `EffortlessMetrics/shipper`.
Follow the [release operator runbook](../release-runbook.md) and do not tag while
the checklist contains a blocking, unavailable, pending, or superseded proof.

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

## Queue Stewardship

Treat every open `shipper-swarm` PR as part of the active development queue:
inspect the intended slice, read CI and review output, fix real findings,
validate honestly, squash-merge when clean, and delete the branch when safe.
Keep overlapping work closed or explicitly deferred before opening another PR
for the same surface.

Dependabot maintenance belongs in `shipper-swarm` first. If the same dependency
bump opens in `EffortlessMetrics/shipper`, close the source-repo PR as duplicate
maintenance work and let the accepted swarm commit flow back through the normal
non-squash source sync.

Factory Droid is retired, so bot-authored and human-authored PRs use the same
current substantive-review requirement, satisfied through Codex, Claude, or a
human reviewer. The required Rust gate evaluates bot diffs through its explicit
GitHub-hosted fallback. When a
maintainer-authored refresh is needed, use the procedure in
[`docs/ci/test-evidence-lanes.md`](../ci/test-evidence-lanes.md) and
[`docs/how-to/shipper-swarm-migration-runbook.md`](../how-to/shipper-swarm-migration-runbook.md):
inspect the diff, run focused validation, push a maintainer-authored refresh or
trusted same-repo branch, and require the normal `Shipper Rust Small Result`
before merge.

For human PRs, GitHub-hosted fallback is an explicit manual proof action, not a
label-triggered refresh. Use the `workflow_dispatch` input `force_route=github`
when self-hosted capacity is unavailable or a cancelled self-hosted lane needs
to be replayed. The required workflow remains restricted to code-changing pull
request events so a label update cannot cancel the synchronize run whose result
branch-protection check is authoritative.

## CI and Branch Protection

`shipper-swarm/main` requires only the normalized result check:

```text
Shipper Rust Small Result
```

Do not require route-specific implementation jobs directly. Only one of those
jobs is expected to run on each attempt.

Current routed Rust-small proof:

- Branch protection requires only `Shipper Rust Small Result`; do not require
  route-specific implementation jobs because exactly one route should run per
  attempt.
- Current same-repo `CPX42` routing proof passed on PR #117 with
  `Routed Rust Small` run `26413038913`; the `CPX42` implementation job and
  normalized `Shipper Rust Small Result` both succeeded.
- Forced route proof before the 100% self-hosted sweep passed for `CX43` with
  `workflow_dispatch` run `26355258014` and for `CX53` with
  `workflow_dispatch` run `26356173639`.
- Current self-hosted fallback proof passed on post-merge `main` run
  `26413498807`; `Shipper Rust Tiny Fallback (routed to self-hosted)` and the
  normalized `Shipper Rust Small Result` both succeeded.
- The router and normalized-result bootstrap jobs run on GitHub-hosted
  infrastructure so an empty self-hosted pool can be detected. Selected Rust
  execution lanes remain self-hosted when available; the explicit fallback
  lane is GitHub-hosted and runs the full Rust-small command list. Do not sync
  this swarm routing policy to `EffortlessMetrics/shipper` until the
  release-authority runner and credential boundaries are explicitly decided.

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
