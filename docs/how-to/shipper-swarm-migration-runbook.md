# Migrate `shipper` to `shipper-swarm` (runbook)

Status: active-development cutover complete.

`EffortlessMetrics/shipper-swarm` is now the active development repository.
`EffortlessMetrics/shipper` remains the release authority for crates.io
publishing, release evidence, and signing credentials until that authority is
explicitly moved.

This runbook defines the recommended migration path from `EffortlessMetrics/shipper` to `EffortlessMetrics/shipper-swarm`.

## Scope and operating constraints

- **Source of truth during migration:** `EffortlessMetrics/shipper` remains the release authority until explicitly moved.
- **Swarm repo shape:** create **public** `EffortlessMetrics/shipper-swarm` with default branch `main`.
- **Trust boundary:** self-hosted runners execute only trusted same-repo PRs (or `workflow_dispatch`); fork PRs go to GitHub-hosted only.
- **Gate style:** branch protection should require only a **normalized result check** after proof sequence passes.
- **Credential boundary:** do not place publish/signing credentials in `shipper-swarm` during initial migration.

## Target state

| Area | Decision |
|---|---|
| Source repo | `EffortlessMetrics/shipper` |
| Swarm repo | `EffortlessMetrics/shipper-swarm` |
| Visibility | Public |
| Default branch | `main` |
| Merge model | Squash merge only in swarm; sync back to `shipper` uses merge commits |
| Required check | `Shipper Rust Small Result` only |
| Release authority | Keep release/publish/signing in `shipper` initially |
| CI routing | CX43 → CX33 → CX53 → GitHub-hosted |
| Fork PR policy | Never run public fork PRs on self-hosted |

## Why start as small/medium lane

`shipper` is a Rust workspace with reliability and publishing logic, but it is not a heavy GPU/model workload. Start on a narrow Rust lane and expand only after measured need.

Primary route:

```text
shipper rust-small:
  CX43 if idle
  CX33 if idle
  CX53 if idle
  GitHub-hosted otherwise
```

Fallback if CX33 proves constrained in real runs:

```text
shipper rust-small:
  CX43 if idle
  CX53 if idle
  GitHub-hosted otherwise
```

## Phase 1 — Create and seed `shipper-swarm`

### 1) Create and configure repository

Configure `EffortlessMetrics/shipper-swarm` with:

- Visibility: public
- Default branch: `main`
- Allow squash merge: yes
- Allow merge commit: no
- Allow rebase merge: no
- Auto-merge: enabled
- Delete branches on merge: enabled
- Branch protection: **off initially**

### 2) Add minimum shared infrastructure

Grant:

- Runner group: `em-ci-small` (selected repositories includes `shipper-swarm`)
- Secret: `EM_RUNNER_READ_TOKEN` (selected repositories includes `shipper-swarm`)

Do **not** add yet:

- `CARGO_REGISTRY_TOKEN`
- crates.io tokens
- release signing secrets
- release/publish tokens

### 3) Seed from `shipper/main`

`shipper-swarm/main` must preserve `shipper/main` ancestry. Do not use an
orphan snapshot seed. The swarm repo should become a branching continuation of
the release-authority repo:

```text
shipper/main:
A---B---C---D

shipper-swarm/main:
A---B---C---D---S1---S2
```

```bash
git clone git@github.com:EffortlessMetrics/shipper-swarm.git
cd shipper-swarm

git remote add public git@github.com:EffortlessMetrics/shipper.git
git fetch public --prune --tags
git fetch origin --prune

git switch -C main public/main
git push --force-with-lease origin main

git fetch origin main
git switch main
git reset --hard origin/main
```

After seeding, verify that `shipper/main` is an ancestor of
`shipper-swarm/main`:

```bash
git merge-base --is-ancestor public/main origin/main
git rev-list --left-right --count public/main...origin/main
```

Expected result:

```text
0 N
```

where `N` is the number of swarm-only commits after the seed point.

After a later non-squash sync from `shipper-swarm` to `shipper`, the source
repo will contain a merge commit that is not yet on `shipper-swarm/main`. Pause
normal swarm PR merges and fast-forward `shipper-swarm/main` to `shipper/main`
before continuing development. That restores the `0 N` ancestry shape without
using orphan snapshots or squashing the sync commit.

## Phase 2 — Add initial routed Rust lane

Add workflow:

- `.github/workflows/em-ci-routed-rust.yml`

First normalized required check:

- `Shipper Rust Small Result`

Do **not** directly require conditional implementation jobs:

- `Route Shipper Rust Small`
- `Shipper Rust Small on CX43`
- `Shipper Rust Small on CX33`
- `Shipper Rust Small on CX53`
- `Shipper Rust Small on GitHub Hosted`

### Initial lane commands

```bash
cargo check --workspace --locked --all-targets
cargo test --workspace --locked --all-targets
cargo test --workspace --locked --doc
cargo run -p shipper -- --help
cargo run -p shipper -- plan --help
cargo run -p shipper -- preflight --help
```

Avoid for first gate:

- real `cargo publish`
- live crates.io credentialed actions
- release packaging/signing
- large matrix/fuzz/full coverage
- network-dependent registry integration

## Phase 3 — Runner routing and observability

### Router targets

Emit one of:

- `cx43`
- `cx33`
- `cx53`
- `github`

### Per-run routing log fields

```text
repo=shipper-swarm
workflow=em-ci-routed-rust
run_id=${{ github.run_id }}
router_target=cx43|cx33|cx53|github
router_reason=cx43_idle|cx33_idle|cx53_idle|no_idle_runner|runner_api_failed|untrusted_pr
```

### Routing policy

- Trusted same-repo PR or `workflow_dispatch`:
  - CX43 if idle
  - CX33 if idle
  - CX53 if idle
  - GitHub-hosted fallback
- Fork PR:
  - GitHub-hosted only
- Release/publish/signing:
  - stays on source repo initially

### CX53 execution shape

```text
--cpus=14
--memory=28g
CARGO_BUILD_JOBS=12

CARGO_HOME=/mnt/ci-cache/cargo-home
SCCACHE_DIR=/mnt/ci-cache/sccache
SCCACHE_CACHE_SIZE=60G
RUSTC_WRAPPER=/usr/local/cargo/bin/sccache
CARGO_INCREMENTAL=0

TMPDIR=/mnt/ci-scratch/tmp/${JOB_ID}
CARGO_TARGET_DIR=/mnt/ci-scratch/target/${JOB_ID}
```

Disk guard checks:

```text
ci-disk-guard /mnt/ci-scratch 100
ci-disk-guard /mnt/docker 20
ci-disk-guard /mnt/ci-cache 20
```

## Phase 4 — Proof sequence before branch protection

Run this sequence:

1. PR that adds routed workflow passes.
2. `workflow_dispatch` on `shipper-swarm/main` passes.
3. Tiny same-repo PR passes.
4. Force CX43 route once.
5. Force CX33 route once (if present).
6. Force CX53 overflow once.
7. Saturate self-hosted and verify GitHub fallback.
8. Verify cleanup + disk reports healthy.

Expected result behavior:

- Exactly one implementation lane runs per attempt.
- Normalized `Shipper Rust Small Result` succeeds in all routing paths.

Then enable branch protection for `main` requiring only:

- `Shipper Rust Small Result`

## Phase 5 — Cut over active development

Recommended cutover order:

1. Announce source freeze for new `shipper` dev.
2. Final fetch `shipper/main`.
3. Re-seed/fast-sync `shipper-swarm/main`.
4. Run routed Rust workflow on `main`.
5. Open tiny same-repo PR to `shipper-swarm`.
6. Confirm normalized result check success.
7. Enable branch protection.
8. Cut over runner access to `shipper-swarm`.
9. Keep source repo as release authority until deliberate migration.

Runner cutover depends on how the current machines are registered:

- If runners are organization-scoped and controlled by a runner group, do not
  reinstall them. Add `EffortlessMetrics/shipper-swarm` to the selected
  repositories for the runner group and verify a routed job is picked up.
- If runners are repository-scoped to `EffortlessMetrics/shipper`, stop the
  runner service, deregister it from the source repo, register it against
  `EffortlessMetrics/shipper-swarm`, and restart the service.

For each self-hosted runner that should serve the swarm repo, keep the routing
labels stable:

```text
self-hosted
Linux
X64
em-ci
rust-small
trusted-pr
cx43 | cx33 | cx53
```

After cutover, confirm each runner appears online and idle for
`EffortlessMetrics/shipper-swarm` before enabling branch protection.

Operator instruction to contributors:

- New `shipper` development targets `EffortlessMetrics/shipper-swarm`.
- Clone side-by-side; do not retarget existing clones in place.
- Do not push to `main` directly.
- Open PRs against `shipper-swarm/main`.
- Wait for `Shipper Rust Small Result`.
- Do not add publish/signing credentials.
- Do not run real publish flows in swarm PR CI.

## Phase 6 — Add additional lanes after burn-in

After 3–5 clean PRs:

| Lane | Route | Purpose |
|---|---|---|
| `Shipper Rust Small Result` | CX43 → CX33 → CX53 → GitHub | Required base gate |
| `Shipper Integration Result` | CX53 → CX43 → GitHub | Fake registry, receipts, resume/reconcile |
| `Shipper Coverage Lite` | GitHub-hosted or manual CX53 | Non-required initially |
| `Shipper Fuzz Smoke` | GitHub-hosted or manual CX53 | Non-required initially |
| `Shipper Release Dry Run` | GitHub-hosted/manual only | No publish credentials |
| Real release/publish | Keep on `shipper` initially | Deliberate later migration |

## Immediate checklist

- [x] Create `EffortlessMetrics/shipper-swarm` as public.
- [x] Enable squash merge + auto-merge + delete branch on merge.
- [x] Add repo to `em-ci-small` selected repositories.
- [x] Add repo to `EM_RUNNER_READ_TOKEN` selected repositories.
- [x] Do **not** add crates.io/release/signing secrets.
- [x] Seed `shipper-swarm/main` from `shipper/main`.
- [x] Add `.github/workflows/em-ci-routed-rust.yml`.
- [x] Route small lane CX43 → CX33 → CX53 → GitHub.
- [x] Guard self-hosted jobs to trusted same-repo work only.
- [x] Include GitHub-hosted fallback.
- [x] Add normalized `Shipper Rust Small Result` job.
- [x] Run `workflow_dispatch` on `main`.
- [x] Open tiny same-repo PR.
- [x] Force fallback-path proof cases.
- [x] Enable branch protection requiring only normalized result.
- [x] Cut runner access over to `shipper-swarm` and verify routed job pickup.
- [x] Move active development to side-by-side `shipper-swarm` clones.

Proof notes:

- PR #2 added the routed Rust small lane.
- PR #3 proved same-repo PR flow through the normalized result check.
- Forced `workflow_dispatch` proof runs covered `cx43`, `cx33`, `cx53`, and
  GitHub-hosted fallback.
- Saturation proof occupied all self-hosted routes and verified auto-routing to
  GitHub-hosted with `router_reason=no_idle_runner`.
- Branch protection for `main` requires only `Shipper Rust Small Result`.
