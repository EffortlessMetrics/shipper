# Test Evidence Lanes

This document maps the configured test evidence lanes for `shipper`: their
triggers, selected checks, and proof scope. See the [Testing Guide](../testing.md)
for local commands and workspace selection. A configured lane is not a passing
receipt: candidate claims require its source SHA, run ID, selected-job results,
and artifacts.

## Doctrine

```
PRs:     routed Rust-small required gate + ripr (advisory) + targeted mutation when risk warrants it
Main:    broad full-CI evidence after merge
Scheduled: weekly broad CI / crypto / mutation; nightly fuzz campaigns
Release: publish / readiness / security proof must be clean to ship
```

`ripr` is the PR-time exposure filter: static mutation-exposure analysis that asks whether changed behavior appears exposed to a meaningful test oracle. It does not run mutants. Full mutation testing is the runtime backstop and belongs in label-gated PRs, weekly schedule, and release lanes — never the default PR hot path.

## Workflow Inventory

Every workflow under `.github/workflows/` and the lane each one occupies. Keep
this inventory aligned when workflows are added, removed, or retargeted.

| Workflow | Trigger | Lane | Required for merge? |
|---|---|---|---|
| `architecture-guard.yml` | `push` + `pull_request` | Every PR | Advisory in swarm branch protection |
| `runner-routing-guard.yml` | `push` + path-scoped `pull_request` + `workflow_dispatch` | Workflow routing policy guard | Advisory |
| `em-ci-routed-rust.yml` | `push` + non-draft `pull_request` / `ready_for_review` + `merge_group` + `workflow_dispatch` | Review-forward required PR gate | Required via `Shipper Rust Small Result` after PR readiness |
| `ci.yml` | `push` + `workflow_dispatch` + weekly `schedule` + trusted review-ready `pull_request` | Main/manual full CI + scoped hosted PR policy | Full CI on main/manual; PR policy evidence is not the default required Rust gate |
| `coverage.yml` | `push` (main) + `pull_request` + `workflow_dispatch` | Advisory / labeled | Advisory |
| `fuzz.yml` | path-scoped `push` + `pull_request`; nightly `schedule` + `workflow_dispatch` | Compile all fuzz binaries; execute selected campaigns only nightly/manually | Advisory |
| `live-runner-interruption-rehearsal.yml` | `workflow_dispatch` + path-scoped same-repository `pull_request` | Safe two-job artifact interruption/resume proof; automatic GitHub-hosted route in swarm | Both selected jobs required when triggered |
| `cross-run-recovery-rehearsal.yml` | Explicit swarm-only `workflow_dispatch` | Three separate seed/retry/finish runs with immediate-predecessor artifact provenance | Manual candidate evidence; not automatic PR proof |
| `mutation.yml` | `schedule` + `workflow_dispatch` + `pull_request` (label-gated) | Weekly + label-gated PR | Advisory |
| `release.yml` | `push` (tags `v*.*.*`) + `workflow_dispatch` | Tag-triggered | Required (when triggered) |
| `ripr.yml` | `pull_request` + `workflow_dispatch` | Advisory (`continue-on-error: true`) | Advisory |

## Required PR Gate

Draft pull requests are review-only for the expensive Rust producer:
creating or editing a draft skips router/producer work. The tiny hosted
`Shipper Rust Small Result` intentionally executes and **fails closed** with
an actionable draft/not-qualified message. GitHub treats skipped required jobs
as successful, so the required result must not be skipped on a draft: a skipped
result could otherwise be mistaken for mergeable proof after the PR becomes
ready. Fork PRs without an eligible trusted producer likewise fail this result.
Marking the same-repository PR ready for review triggers a complete new required
producer/result on that source, including when the commit SHA is unchanged.
After any ready-PR update, require its latest exact-head successful Result; a
draft-time failure never qualifies the candidate. Push, merge-group and manual
routes retain their prior behavior. Already-running jobs are not retroactively
cancelled when a PR becomes draft. The regression is
`cargo test -p xtask --test routed_draft_admission --locked`.

`em-ci-routed-rust.yml` is the required PR workflow. Branch protection requires
only the normalized `Shipper Rust Small Result` check.

In `EffortlessMetrics/shipper-swarm`, trusted same-repo PRs route through
self-hosted runners in this order:

```text
CX43 -> CPX42 -> CX53
```

The router and normalized-result bootstrap jobs run on GitHub-hosted
infrastructure so they can observe an empty self-hosted pool. The selected
Rust execution lane remains self-hosted when capacity is available. When no
approved runner is idle, the router explicitly selects the full GitHub-hosted
fallback lane; this is not a silent fallback or a label-triggered bypass.

Public fork PRs are denied by the normalized result instead of running
repository code on self-hosted runners. A maintainer can move trusted work onto
a same-repo branch when it needs the swarm gate.

Dependabot PRs are same-repo dependency maintenance. Their bot-authored runs do
not receive the runner-read secret, so the router automatically selects the
secret-free GitHub-hosted Rust-small lane with
`router_reason=bot_pr_github_fallback` and `fallback_allowed=true`. This is the
normal bot-PR gate path, not a pre-evaluation failure or a reason to expose the
secret. A maintainer-authored refresh is needed only when the normalized gate
actually fails or the candidate requires a repair:

```text
1. Inspect the dependency diff and confirm it is narrow.
2. Run focused local validation such as `cargo check --workspace --locked`.
3. If the gate fails or the candidate needs repair, add a maintainer-authored
   commit or recreate the bump on a trusted same-repo branch.
4. Require the normal `Shipper Rust Small Result`, a current substantive review
   through Codex, Claude, or a human reviewer, and any focused dependency
   validation before merge.
```

This is not a fallback-policy exception. Do not add release credentials, crates.io
tokens, signing secrets, or broad bot secret access to make dependency PRs pass.

Do not infer release-authority behavior from this swarm routing policy.
`EffortlessMetrics/shipper` remains the release authority, and the broad
`shipper-swarm` self-hosted sweep must not be synced there until release
runner, binary-build, and credential boundaries are explicitly decided.

The self-hosted Rust-small lane proves:

```bash
cargo fmt --all -- --check
cargo check --workspace --locked --all-targets
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo nextest run --workspace --locked --all-targets --all-features --profile ci
cargo test --workspace --locked --doc
cargo package --workspace --locked --exclude xtask
cargo run -p shipper -- --help
cargo run -p shipper -- plan --help
cargo run -p shipper -- preflight --help
```

`cargo package` proves each crate builds from its own `.crate` tarball with
path dependencies resolved through version requirements — the check the
`install-smoke` lane structurally cannot make, because `cargo install --path`
resolves path deps directly.

`cargo fmt --check` and `cargo clippy` run here — not only in `ci.yml`'s
`lint` job — because its narrow PR trigger now runs **only the hosted policy**
job, not full-CI lint. Without these commands in Rust-small, lint regressions
would reach main before full CI catches them.

The GitHub-hosted fallback lane runs the same command list. It used to
prove less (`cargo check` + `--help` only); that was reverted after a
regression escaped through it, and the fallback now carries the full
gate. Only the runner differs.

## `ci.yml` — Full-CI Lane Map

`ci.yml` runs broad full CI on pushes to `main`, manual `workflow_dispatch`,
and Sunday at 03:00 UTC (`0 3 * * 0`). It also has a **narrow** non-draft,
trusted PR event solely for the existing hosted policy job; none of the twelve
full-CI default/manual suites becomes a PR-triggered job. The
[workflow source](../../.github/workflows/ci.yml) owns these predicates and
commands. The table names its default Linux self-hosted jobs;
explicit manual hosted selection substitutes the counterparts described below.
Every selected job must complete successfully before its configured scope can
be claimed as passing evidence.

| Job | Predicate | Configured check |
|---|---|---|
| `lint` | `push` / `workflow_dispatch` / `schedule` | `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`. |
| `policy` | `push` / `workflow_dispatch` / `schedule` | Seven policy checks in `--mode blocking-allowlist`, advisory doc contracts, and `policy-report`. See [non-Rust rollout](../policy/NON_RUST_ROLLOUT.md). |
| `test` (nextest) | `push` / `workflow_dispatch` / `schedule` | `cargo nextest run --workspace --all-features --profile ci` with `INSTA_UPDATE=no`, `PROPTEST_CASES=16`, and `PROPTEST_MAX_SHRINK_ITERS=1000`; then `cargo test --workspace --doc`. Execution is on Linux. |
| `install-smoke` | `push` / `workflow_dispatch` / `schedule` | Install the `shipper` facade from its local path, then run version/help commands. This is separate from the PR lane's `.crate` package verification. |
| `crypto-proptests-heavy` | `push` / `workflow_dispatch` / `schedule` | `cargo nextest run -p shipper-encrypt --all-features --profile ci` with `PROPTEST_CASES=256`; only this crate receives the heavier job. |
| `msrv` | `push` / `workflow_dispatch` / `schedule` | `cargo check --workspace` on Rust 1.95.0. |
| `security` | `push` / `workflow_dispatch` / `schedule` | `cargo audit` against the advisory database fetched by the run. |
| `docs` | `push` / `workflow_dispatch` / `schedule` | `cargo doc --workspace --no-deps --document-private-items` with `RUSTDOCFLAGS=-Dwarnings`. |
| `bdd` | `push` / `workflow_dispatch` / `schedule` | Run the `bdd_publish`, `bdd_resume`, and `e2e_rehearse` CLI integration binaries. |
| `fuzz-smoke` | `push` / `workflow_dispatch` | Five targets with a 60-second budget each: `load_state`, `resolve_token`, `schema_version`, `release_levels`, and `redact_output`. |
| `cross-platform` | `push` / `workflow_dispatch` / `schedule` | `cargo check --workspace --target x86_64-unknown-linux-gnu`. The current matrix contains only that Linux target and does not execute tests. |
| `release-build` | `push` / `workflow_dispatch` | `cargo build --release` for the default members, followed by a Linux CLI artifact upload. This is release-profile compilation, not publication. |

Broad full-CI remains part of the evidence story, but it is not the merge gate
for ordinary PRs. The required PR signal is the routed result above.

## Manual Full-CI Proof for a PR

Most PRs should merge on `Shipper Rust Small Result` plus focused local or
review evidence. Use extra full-CI proof when a PR touches release-critical
behavior, runner policy, workflow routing, broad state/event/receipt contracts,
or another surface where post-merge discovery would be too expensive.

For same-repo branches, run the full lane before merge with:

```text
Actions -> CI -> Run workflow -> branch: <pr-branch>
```

That manual dispatch runs `ci.yml` on the selected branch. It exercises the
broad self-hosted evidence set, including nextest, doc tests, policy, BDD,
fuzz smoke, heavy crypto proptests, native Linux target check, and release
build. It does not publish crates or move release authority into
`shipper-swarm`.

When self-hosted allocation is unavailable, `shipper-swarm` maintainers can
explicitly select the manual hosted route:

```bash
gh workflow run ci.yml --repo EffortlessMetrics/shipper-swarm \
  --ref <reviewed-branch> -f runner_route=github-hosted
```

The default `runner_route=self-hosted` preserves the existing job groups and
labels. Hosted selection applies only to `workflow_dispatch` in
`EffortlessMetrics/shipper-swarm`; push and schedule behavior stays governed by
the existing route. This option does not apply to the release authority.

All twelve hosted jobs reuse the original step sequences through YAML anchors,
including full-strength crypto, policy, security, MSRV, BDD, fuzz smoke, and
release-profile compilation. Their dependency edges select the hosted jobs,
their caches have a separate namespace, and their concurrency group prevents
a later default main run from cancelling deliberate manual proof. The unused
self-hosted counterparts are intentionally skipped on that run; every selected
hosted job must execute and reach a terminal result.

This is an opt-in broad proof run and can exceed the ordinary PR CI cost target.
Dispatch once after the candidate and route have been reviewed; do not repeatedly
dispatch an unchanged failure. Retain the run ID, source SHA, per-job results,
and artifacts. Passing this route does not establish self-hosted availability,
publication authorization, or the remaining exact-candidate release matrix.
The [manual route plan](../../plans/swarm/manual-full-ci-hosted.md) and
`python3 scripts/ci/check-full-ci-hosted.py --test` define the bounded contract.

For PR-time advisory evidence, a maintainer can also apply labels:

The canonical names, descriptions, colors, and lane memberships live in
`policy/ci-trigger-labels.toml`. Validate source/workflow agreement offline with
`cargo xtask ci-labels check`, compare live GitHub metadata read-only with
`cargo xtask ci-labels check-live --repo EffortlessMetrics/shipper-swarm`, and
use `cargo xtask ci-labels sync --repo EffortlessMetrics/shipper-swarm --apply`
only as an explicitly reviewed maintainer action. Sync prints its deterministic
plan before applying it, creates or updates only the configured labels, and
never deletes unrelated labels. GitHub label updates are not transactional: a
failed apply may leave an already-applied bounded update, and an idempotent
rerun converges the remaining drift.

| Label | Effect |
|---|---|
| `coverage` | Selects `coverage.yml` for the next code-changing PR event. Label-only events deliberately do not launch/cancel coverage; `workflow_dispatch` is a separate unconditional path for an immediate run and does not consume the label. |
| `mutation` | Runs the label-gated `mutants-pr` job on changed production Rust files. |
| `full-ci` | Selects both advisory lanes asymmetrically: mutation can start immediately when the label is applied, while coverage waits for the next code-changing PR event. For an immediate combined run, apply the label for mutation and separately dispatch coverage; that dispatch is unconditional. `full-ci` never triggers broad `ci.yml`. |

Do not run same-repo self-hosted full-CI proof on untrusted fork code. Move the
work onto a trusted same-repo branch first, or rely on the fork-safe normalized
result behavior until a maintainer has reviewed the code.

When manual full-CI proof is used as merge evidence, record the workflow run ID
in the PR so future queue stewards can audit what was proven.

## Policy Gates (xtask-Enforced, Inside `ci.yml`'s `policy` Job)

The `policy` job runs each check in blocking-allowlist mode and uploads `target/policy/` as an artifact regardless of outcome.

| Check | What it proves | Introduced |
|---|---|---|
| `cargo xtask check-file-policy --mode blocking-allowlist` | All tracked non-Rust files are receipted in `policy/non-rust-allowlist.toml`. | #210 |
| `cargo xtask check-generated` | Receipts for `policy/no-panic-baseline.json` and `badges/*.json` are present and valid. | #209 / #182 PR 2 |
| `cargo xtask check-executable-files` | Tracked executable files match `policy/executable-allowlist.toml`. | #209 |
| `cargo xtask check-dependency-surfaces` | Cargo manifests match `policy/dependency-surface-allowlist.toml`. | #209 |
| `cargo xtask check-workflow-surfaces` | `.github/workflows/*.yml` files match `policy/workflow-allowlist.toml`. | #210 |
| `cargo xtask check-process-policy` | Workflow `run:` commands stay inside their declared process profile. | #211 |
| `cargo xtask check-network-policy` | Workflow endpoints stay inside their declared network profile. | #211 |
| `cargo xtask check-lint-policy` | MSRV agrees across `Cargo.toml`, `clippy.toml`, `policy/clippy-lints.toml`; every `[workspace.lints.clippy]` entry has a ledger entry. | #179 / #191 |
| `cargo xtask no-panic check --mode blocking` | No new panic-family debt since `policy/no-panic-baseline.json`. Runs in `release.yml` policy-gate today (see Release Proof). | #187 |

## Advisory / Routed (PRs only)

| Job | Workflow | Trigger | What it proves |
|---|---|---|---|
| `coverage` | `coverage.yml` | `push` to main, dispatch, or a code-changing PR event while `coverage`/`full-ci` is present | Codecov line/branch coverage. Label-only events deliberately do not start or cancel this lane. |
| `rust-small` | `em-ci-routed-rust.yml` | PRs, merge groups, pushes to main, dispatch | Required Rust-small PR gate with self-hosted routing and explicit fallback control. Carries `cargo fmt --check` and `cargo clippy -- -D warnings`, because `ci.yml`'s `lint` job never sees a pull request. |
| `ripr-pilot` | `ripr.yml` | PRs touching `crates/**`, `xtask/**`, `Cargo.{toml,lock}`, `ripr.toml`, `policy/ripr-suppressions.toml`, `.github/workflows/ripr.yml`. `continue-on-error: true`. | Static mutation-exposure analysis: does the diff appear exposed to a meaningful test oracle? |
| `mutants-pr` | `mutation.yml` | PRs labeled `mutation` or `full-ci` | Runtime mutation backstop scoped to the PR's changed files via `cargo xtask mutants-pr --changed`. Blocking when it runs. |

## Scheduled

| Job | Schedule | What it proves |
|---|---|---|
| `fuzz` matrix (6 targets) | `fuzz.yml` — daily | Extended fuzz energy beyond the PR smoke pass. |
| `crypto-proptests-heavy` | `ci.yml` — on `push` / `workflow_dispatch` / `schedule` | Full-strength `proptest` for `shipper-encrypt` round-trips (90-minute budget). |
| `mutants-weekly` | `mutation.yml` — Sunday 04:00 UTC | Mutation score across `shipper-duration` / `shipper-types` / `shipper-config`. Expanding to full trust-critical surface is a future rollout step (60-minute budget today). |

## Targeted Mutation on a PR (Label-Gated)

When a maintainer applies the `mutation` or `full-ci` label, `mutation.yml`'s `mutants-pr` job runs:

```bash
cargo xtask mutants-pr --changed --base origin/<PR-base>
```

The wrapper computes `git diff <base>...HEAD --name-only -- '*.rs'`, filters out `tests/` and `benches/` paths (cargo-mutants only mutates production source), and runs `cargo mutants --no-shuffle --workspace --file <each>`. Workspace selection makes non-default members discoverable; the retained `--file` arguments still limit mutation to the PR's changed production files. A `--dry-run` mode (`cargo mutants --list`) is available locally for shape inspection without running tests. When no production Rust files changed, the wrapper exits before probing or invoking cargo-mutants.

Local invocation:

```bash
# Inspect which mutants the PR would generate, no tests run.
cargo xtask mutants-pr --changed --dry-run

# Real run.
cargo xtask mutants-pr --changed
```

If `cargo-mutants` is missing locally, the wrapper prints install instructions and exits advisory-success rather than erroring; CI installs the tool before invoking.

A maintainer should apply the label when:

- Changes touch `shipper-core` publish/reconcile/readiness, `shipper-encrypt`, `shipper-output-sanitizer`, `shipper-cargo-failure`, `shipper-sparse-index`, `shipper-webhook`, or state/event/receipt types.
- `ripr` raises a `warning`-level finding in a trust-critical crate that benefits from execution-backed confirmation.

## Release Proof

`release.yml` proves end-to-end publication safety. Triggers on `v*.*.*` tag push or `workflow_dispatch`.

| Job (release.yml) | What it proves |
|---|---|
| `build-binaries` | Linux/Windows/macOS release binaries produced (one matrix leg per target). |
| `msrv-gate` | `cargo check --workspace` on declared MSRV. |
| `policy-gate` | `cargo xtask no-panic check --mode blocking` + `cargo xtask check-lint-policy`. Blocks publish if either ledger has drifted since the SHA the baseline was generated at. (#187) |
| `publish-crates-io` | Dogfoods Shipper itself: `shipper plan` → `shipper preflight` → `shipper publish`. Trusted Publishing via OIDC; falls back to `CARGO_REGISTRY_TOKEN`. Idempotent — resumes from `.shipper/state.json` on rerun. |
| `create-release` | Attaches platform binaries and `.shipper/` state to the GitHub Release. Runs only after `publish-crates-io` succeeds. |
| `release-rehearse` | `workflow_dispatch mode=rehearse`: plan + preflight dry-run only. Useful before cutting a tag. |
| `release-resume` | `workflow_dispatch mode=resume`: re-enters the publish train from a prior run's `shipper-state-final` artifact. Plan-ID match required. |

The live recover rehearsal remains a manual release-candidate procedure because
it intentionally publishes throwaway crates.io versions and then yanks them.
The full-CI proof is the synthetic side: `ci.yml` runs
`cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`, which exercises
a real `shipper publish` interruption/resume sequence against fake Cargo and a
mock registry, then checks `state.json`, append-only `events.jsonl`, skipped
published crates, and duplicate-publish invariants.

### Runner artifact interruption/resume proof

`live-runner-interruption-rehearsal.yml` exercises a separate boundary from the
full-CI BDD run: one job uploads interrupted `.shipper` evidence and a second
job downloads that artifact before resuming. In `EffortlessMetrics/shipper-swarm`,
same-repository path-triggered PRs and manual dispatches automatically select
`interrupt-hosted` and `resume-hosted` on ephemeral Ubuntu runners. Other
repositories retain the existing scoped self-hosted pair. Fork PRs execute
neither pair. Shared step anchors preserve the same ignored Cargo tests,
loopback registry, artifact names and hidden-file transfer across routes.

When this workflow is triggered, both selected jobs must succeed on the current
candidate or its verified synthetic merge subject. Verify the run's source
identity, nonempty seed and resumed artifacts, and successful download and
resume test. The unused route is deliberately skipped; a queued, cancelled,
failed, or missing selected job is not passing evidence. Local ZIP-transfer
proof and the ordinary `e2e_rehearse` tests do not replace this GitHub artifact
handoff. The two hosted jobs have separate 30-minute limits and isolated cache
keys. `python3 scripts/ci/check-live-interruption-routing.py --test` checks the
route, dependency, and artifact contract with accepting/rejecting fixtures.

This uses fake Cargo and a mock registry with read-only workflow permissions.
It establishes runner-artifact recovery evidence, not crates.io publication or
release-authority authorization. See [#380](https://github.com/EffortlessMetrics/shipper-swarm/issues/380).

## Evidence Composition

A complete evidence picture for a release requires all of the following:

| Evidence | Source |
|---|---|
| Required PR gate | `em-ci-routed-rust.yml` `Shipper Rust Small Result` |
| Workspace tests pass | `ci.yml` `test` lane on the self-hosted runner pool for main/manual/weekly runs |
| Native Linux target check compiles | `ci.yml` `cross-platform` lane for x86_64 Linux on main/manual/weekly runs |
| No known vulnerabilities | `ci.yml` `security` (`cargo audit`) on main/manual/weekly runs |
| No architectural drift | `architecture-guard.yml` |
| Format clean | `ci.yml` `lint` on main/manual/weekly runs |
| Clippy clean | `ci.yml` `lint` (`cargo clippy -- -D warnings`) on main/manual/weekly runs |
| MSRV verified | `ci.yml` `msrv` + `release.yml` `msrv-gate` |
| BDD scenarios pass | `ci.yml` `bdd` on main/manual/weekly runs |
| No panic-family debt added | `release.yml` `policy-gate` (`no-panic check --mode blocking`) |
| Policy gates green | `ci.yml` `policy` (every xtask check in blocking-allowlist) |
| Static exposure signal | `ripr.yml` `ripr-pilot` (advisory) |
| Mutation signal (opt-in) | `mutation.yml` `mutants-pr` (label-gated) or `mutants-weekly` |
| Coverage signal (opt-in) | `coverage.yml` (selected by `coverage`/`full-ci` on the next code-changing PR event; dispatch independent) |
| Publish path verified | `release.yml` `publish-crates-io` (dogfoods Shipper) |
| Trusted Publishing configured | OIDC token exchange in `release.yml` |

## Trust-Critical Crates

These crates receive the most rigorous mutation coverage because they handle real registry, state, and security operations:

| Crate | Risk |
|---|---|
| `shipper-core` | Publish engine, reconcile, resume, plan |
| `shipper-types` | Shared state/event types |
| `shipper-encrypt` | Token encryption |
| `shipper-output-sanitizer` | Token redaction in logs |
| `shipper-cargo-failure` | Cargo exit-code / stderr classification |
| `shipper-sparse-index` | Registry sparse-index parsing |
| `shipper-registry` | Registry API interactions |
| `shipper-cli` | CLI dispatch, output |
| `shipper` | Install façade |

## Routing Follow-Ups

The current swarm posture optimizes for a reliable required PR signal first:
`Shipper Rust Small Result`. Broad full-CI still runs after merge on `main`, on
manual dispatch, and on the weekly schedule.

Concrete follow-up candidates:

1. Repeat forced route proof and refresh the proof ledger whenever route order,
   runner labels, fallback policy, or the normalized-result contract changes.

Resolved follow-up:

- `architecture-guard.yml` remains advisory in `shipper-swarm` branch
  protection. On 2026-05-25, `gh api
  repos/EffortlessMetrics/shipper-swarm/branches/main/protection/required_status_checks`
  returned only `Shipper Rust Small Result`. The architecture guard still runs
  as PR/push evidence, but it is not a separate required status check.

### Review-ready policy proof

On non-draft same-repository PRs to main, including same-SHA
`ready_for_review` transitions, `CI` executes only its original hosted
`policy_steps` job. It runs seven blocking allowlist commands, advisory
document contracts, and the retained policy report. The other eleven
hosted jobs require deliberate twelve-job manual dispatch, and all normal
self-hosted full-CI jobs are excluded from PR events. Skipped draft/fork
policy jobs are **not** successful proof. The exact source checker is
`python3 scripts/ci/check-full-ci-hosted.py --test`.
Staged `cargo precommit` requires a real authoring index and is not claimed.

### Three independent recovery runs

`cross-run-recovery-rehearsal.yml` is an opt-in companion, not a replacement for
the automatic two-job rehearsal. Dispatch `seed` with no predecessor, then
`retry` with that successful harness run ID, then `finish` with the successful
retry run ID. Keep the selected source SHA identical across all three runs.
A controlled incomplete publish is a successful fixture phase, not whole-plan
completion; the middle stage must have no final receipt.

The helper in `scripts/ci/test-release-resume-artifacts.py` defaults to offline
regressions. Its explicit `prepare` mode validates the checkout using the existing
release source verifier and executes the unchanged production release selector.
Read-only Actions metadata, artifact ID/digest, exact source/tree, run attempt,
phase, fixture registry/version/root, and every checkpoint-file digest must agree
before restoration. `record` checks actual upload counts, cumulative attempts,
execution segments and the exact predecessor event prefix. Harness provenance is
retained separately as `cross-run-harness.json`; it is not a public product schema
or release authorization. The canonical `shipper-state-final` upload includes
hidden files and runs even after failure; a failed harness run cannot authorize
continuation. No recursive dispatch, release environment, OIDC or publishing
credentials are used. The existing release and CI process profiles are unchanged.

After reviewed implementation lands, one purposeful three-run chain must retain
three distinct run IDs, the selected artifact IDs/digests, and the second run as
the final run's predecessor. Add an actual stale-seed final-stage refusal and
retain unchanged-input evidence. Local synthetic transport tests, green source
checks and configured workflow YAML do not establish that hosted proof. The
release-authority consumption and public-finalization obligations remain under
#418 and the separate release process. This rehearsal uses fake Cargo and the
fixed loopback registry, never a real registry publication.
