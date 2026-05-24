# Test Evidence Lanes

This document maps the test evidence strategy for `shipper`: which lanes run when, what each lane proves, and how they compose into a complete evidence picture.

## Doctrine

```
PRs:     routed Rust-small required gate + ripr (advisory) + targeted mutation when risk warrants it
Main:    broad full-CI evidence after merge
Weekly:  deeper fuzz / proptest / mutation lanes
Release: publish / readiness / security proof must be clean to ship
```

`ripr` is the PR-time exposure filter: static mutation-exposure analysis that asks whether changed behavior appears exposed to a meaningful test oracle. It does not run mutants. Full mutation testing is the runtime backstop and belongs in label-gated PRs, weekly schedule, and release lanes — never the default PR hot path.

## Workflow Inventory

Every workflow under `.github/workflows/` and the lane each one occupies. The audit dates from the #189 docs pass.

| Workflow | Trigger | Lane | Required for merge? |
|---|---|---|---|
| `architecture-guard.yml` | `push` + `pull_request` | Every PR | Advisory in swarm branch protection |
| `em-ci-routed-rust.yml` | `push` + `pull_request` + `merge_group` + `workflow_dispatch` | Required PR gate | Required via `Shipper Rust Small Result` |
| `ci.yml` | `push` + `workflow_dispatch` + weekly `schedule` | Main/manual full-CI evidence + weekly heavy proptest | Required when triggered; not the default PR gate |
| `coverage.yml` | `push` (main) + `pull_request` + `workflow_dispatch` | Advisory / labeled | Advisory |
| `droid-review.yml` | `pull_request` | Advisory (same-repo + bot guard) | Advisory |
| `droid.yml` | `issues` + `pull_request` (command-triggered) | Advisory (trusted-actor guard) | Advisory |
| `droid-security-scan.yml` | `schedule` + `workflow_dispatch` | Scheduled (Mon 08:00 UTC) | Advisory |
| `fuzz.yml` | `schedule` + `workflow_dispatch` | Nightly | Advisory |
| `mutation.yml` | `schedule` + `workflow_dispatch` + `pull_request` (label-gated) | Weekly + label-gated PR | Advisory |
| `release.yml` | `push` (tags `v*.*.*`) + `workflow_dispatch` | Tag-triggered | Required (when triggered) |
| `ripr.yml` | `pull_request` + `workflow_dispatch` | Advisory (`continue-on-error: true`) | Advisory |

## Required PR Gate

`em-ci-routed-rust.yml` is the required PR workflow. Branch protection requires
only the normalized `Shipper Rust Small Result` check.

In `EffortlessMetrics/shipper-swarm`, trusted same-repo PRs route through
self-hosted runners in this order:

```text
CPX42 -> CX43 -> CX53
```

Fork or explicitly allowed fallback paths use the GitHub-hosted tiny fallback.
Silent fallback is blocked: if self-hosted routing is unavailable for a trusted
same-repo PR or merge-queue run in `shipper-swarm`, the normalized result fails
unless an operator has explicitly applied `allow-github-hosted` or
`ci-budget-ack`, or forced the `github` route through `workflow_dispatch`.

In `EffortlessMetrics/shipper`, the same workflow intentionally takes the
GitHub-hosted tiny fallback. The source repository remains the release authority
and does not carry the swarm runner-routing secret; source sync and
release-authority PRs should not depend on `shipper-swarm` self-hosted routing.

The self-hosted Rust-small lane proves:

```bash
cargo check --workspace --locked --all-targets
cargo nextest run --workspace --locked --all-targets --all-features --profile ci
cargo test --workspace --locked --doc
cargo run -p shipper -- --help
cargo run -p shipper -- plan --help
cargo run -p shipper -- preflight --help
```

The GitHub-hosted tiny fallback intentionally proves less:

```bash
cargo check --workspace --locked --all-targets
cargo run -p shipper -- --help
cargo run -p shipper -- plan --help
```

## `ci.yml` — Full-CI Lane Map

`ci.yml` is the broad full-CI workflow. It no longer runs on every PR. It runs
on pushes to `main`, manual `workflow_dispatch`, and the weekly schedule. Every
entry below is required when `ci.yml` is triggered unless the `Predicate`
column carries a gate.

| Job | Predicate | Wall clock | What it proves |
|---|---|---|---|
| `lint` | `push` / `workflow_dispatch` / `schedule` | ~1 min | `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings`. |
| `policy` | `push` / `workflow_dispatch` / `schedule` | ~1 min | All seven xtask policy checks in `--mode blocking-allowlist`, plus `policy-report`. See `docs/policy/NON_RUST_ROLLOUT.md`. |
| `test` (nextest, 3-OS matrix) | `push` / `workflow_dispatch` / `schedule` | ~17 min (longest leg) | Unit and integration tests pass on Ubuntu, Windows, macOS. Doc-tests run alongside. |
| `crypto-proptests-heavy` | `schedule` / `push` / `workflow_dispatch` only | ~20 min | Full-strength `proptest` for `shipper-encrypt` round-trips. **Not** on PR — too slow. |
| `msrv` | `push` / `workflow_dispatch` / `schedule` | ~1 min | `cargo check --workspace` on the declared MSRV (1.95). |
| `security` | `push` / `workflow_dispatch` / `schedule` | ~1 min | `cargo audit` against the current advisory database. |
| `docs` | `push` / `workflow_dispatch` / `schedule` | ~1 min | `cargo doc --workspace --no-deps` clean under `-D warnings` (catches `rustdoc::invalid-html-tags` and friends). |
| `bdd` | `push` / `workflow_dispatch` / `schedule` | ~3 min | Publish and resume BDD scenarios plus the synthetic interruption-resume rehearsal (`e2e_rehearse`) that proves persisted state/events let `shipper resume` complete without duplicate publishes. |
| `fuzz-smoke` | `push` / `workflow_dispatch` | ~10 min | Five fuzz targets at low energy: load state, resolve token, schema version, release levels, and output redaction. |
| `cross-platform` | `push` / `workflow_dispatch` / `schedule` | ~2 min per leg | Multi-target builds: Linux x86_64/aarch64, Windows MSVC, macOS x86_64/aarch64. |
| `release-build` | `push` / `workflow_dispatch` only | ~2 min | Release-profile build (LTO + strip) remains available on main and manual runs; tag-time binaries are built by `release.yml`. |

Broad full-CI remains part of the evidence story, but it is not the merge gate
for ordinary PRs. The required PR signal is the routed result above.

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
| `coverage` | `coverage.yml` | `push` to main, dispatch, `coverage` or `full-ci` label on PR | Codecov line/branch coverage. |
| `rust-small` | `em-ci-routed-rust.yml` | PRs, merge groups, pushes to main, dispatch | Required Rust-small PR gate with self-hosted routing and explicit fallback control. |
| `ripr-pilot` | `ripr.yml` | PRs touching `crates/**`, `xtask/**`, `Cargo.{toml,lock}`, `ripr.toml`, `policy/ripr-suppressions.toml`, `.github/workflows/ripr.yml`. `continue-on-error: true`. | Static mutation-exposure analysis: does the diff appear exposed to a meaningful test oracle? |
| `mutants-pr` | `mutation.yml` | PRs labeled `mutation` or `full-ci` | Runtime mutation backstop scoped to the PR's changed files via `cargo xtask mutants-pr --changed`. Blocking when it runs. |
| `droid-review` | `droid-review.yml` | Same-repo PRs (incl. `dependabot[bot]`). | Automated code review via Factory Droid (BYOK MiniMax M2.7). Advisory comments, no merge gate. |
| `droid` | `droid.yml` | `@droid` mentions on issues / PRs by `OWNER`/`MEMBER`/`COLLABORATOR`. | On-demand Droid actions: review, refactor, explain. |

## Scheduled

| Job | Schedule | What it proves |
|---|---|---|
| `fuzz` matrix (6 targets) | `fuzz.yml` — daily | Extended fuzz energy beyond the PR smoke pass. |
| `crypto-proptests-heavy` | `ci.yml` — on `push`/`schedule`/`workflow_dispatch` | Full-strength `proptest` for `shipper-encrypt`. |
| `mutants-weekly` | `mutation.yml` — Sunday 04:00 UTC | Mutation score across `shipper-duration` / `shipper-types` / `shipper-config`. Expanding to full trust-critical surface is a future rollout step (60-minute budget today). |
| `droid-security-scan` | `droid-security-scan.yml` — Monday 08:00 UTC | Factory Droid security scan, 7-day window, medium threshold, critical blocking. |

## Targeted Mutation on a PR (Label-Gated)

When a maintainer applies the `mutation` or `full-ci` label, `mutation.yml`'s `mutants-pr` job runs:

```bash
cargo xtask mutants-pr --changed --base origin/<PR-base>
```

The wrapper computes `git diff <base>...HEAD --name-only -- '*.rs'`, filters out `tests/` and `benches/` paths (cargo-mutants only mutates production source), and runs `cargo mutants --no-shuffle --file <each>`. A `--dry-run` mode (`cargo mutants --list`) is available locally for shape inspection without running tests.

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

## Evidence Composition

A complete evidence picture for a release requires all of the following:

| Evidence | Source |
|---|---|
| Required PR gate | `em-ci-routed-rust.yml` `Shipper Rust Small Result` |
| Tests pass on all platforms | `ci.yml` `test` matrix (Ubuntu, Windows, macOS) on main/manual/weekly runs |
| Multi-target builds compile | `ci.yml` `cross-platform` matrix (5 targets) on main/manual/weekly runs |
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
| Coverage signal (opt-in) | `coverage.yml` (label-triggered) |
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

1. Add an explicit `full-ci` label or manual dispatch recipe for PRs that need
   full matrix proof before merge.
2. Record routed proof IDs for CPX42, CX43, CX53, and explicit GitHub-hosted
   fallback after the #72 routing change.
3. Revisit whether `architecture-guard.yml` should remain separately required
   once the routed Rust-small lane is proven stable under the new settings.
