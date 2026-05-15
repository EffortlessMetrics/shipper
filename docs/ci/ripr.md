# ripr — Static Mutation-Exposure Analysis

This document describes the `ripr` advisory lane for `shipper`: what it does, when it runs, what it means, and how to act on its findings.

`ripr` is the external [EffortlessMetrics/ripr](https://github.com/EffortlessMetrics/ripr) CLI (`crates.io/crates/ripr`). Shipper consumes ripr as an advisory PR lane; Shipper does not embed ripr's analysis. The configuration lives in `ripr.toml` at the workspace root, and the wrapper subcommand is `cargo xtask ripr-pr`.

## What ripr Does

`ripr` is **static mutation-exposure analysis**. It reads a PR diff, builds mutation-shaped probes from the changed behavior, and asks whether the existing tests appear to expose that behavior to a meaningful discriminator. It does **not** find or run actual mutants — mutation testing remains the runtime backstop, scoped to targeted/nightly/release lanes.

The question ripr answers at draft time:

```text
For the behavior changed in this diff, do the current tests include
an assertion or check that would catch the changed behavior?
```

Under the hood ripr uses the RIPR model — **Reachability**, **Infection**, **Propagation**, **Revealability** — to classify each probe's exposure evidence.

The three tiers, side by side:

| Tier | Question |
|---|---|
| **Coverage** | Did this code execute under the test suite? |
| **ripr** | Does the changed behavior appear exposed to a meaningful test oracle? |
| **Mutation testing** | Did the tests fail when a concrete mutant was run? |

## Advisory Status

`ripr` is advisory. A failing `ripr` report does not block merge. It is a signal, not a gate.

Suppressions for findings that are intentionally untested are receipted in `policy/ripr-suppressions.toml`. Each suppression requires a reason and an owner.

## When ripr Runs

The `ripr` workflow triggers on PRs that touch:

```
crates/**
xtask/**
Cargo.toml
Cargo.lock
ripr.toml
policy/ripr-suppressions.toml
.github/workflows/ripr.yml
```

It also runs on `workflow_dispatch`.

## Concurrency

The ripr job cancels in-progress runs for the same PR when a new commit is pushed. A new commit supersedes the old report.

## How to Read a ripr Report

ripr produces both human-readable markdown summaries and structured JSON (`repo-exposure-json` and SARIF variants). The PR-time pilot writes its outputs under `target/ripr/pilot/`.

Findings classify each probe's evidence via `[severity.findings]` in `ripr.toml`. The canonical severity assignments (out of the box):

| Finding shape | Default severity |
|---|---|
| `exposed` | `info` |
| `weakly_exposed` | `warning` |
| `reachable_unrevealed` | `warning` |
| `no_static_path` | `warning` |
| `infection_unknown` | `warning` |
| `propagation_unknown` | `note` |
| `static_unknown` | `note` |

ripr also classifies seam-level grip via `[severity.seams]` (off/info/warning/note across `weakly_gripped`, `ungripped`, `reachable_unrevealed`, etc.). The pilot's "Top recommendation" surfaces the highest-leverage seam first.

## When to Act on ripr Findings

**Act on `warning` findings in trust-critical crates first:** if the changed behavior is `weakly_exposed` or `reachable_unrevealed` and lives in `shipper-core`, `shipper-encrypt`, `shipper-output-sanitizer`, `shipper-cargo-failure`, `shipper-sparse-index`, or `shipper-registry`, write or strengthen the test, or add a suppression with justification.

**Consider acting on warnings elsewhere:** if a gap is small and a test is cheap to write, write it. If the path is covered by BDD or integration tests not visible to ripr, note this in the suppression receipt.

**`info` and `note` findings are advisory context.** They are not failures; review them when convenient.

## Triggering Full Mutation

If `ripr` raises a `warning`-level finding in a trust-critical crate that you want execution-backed confirmation for, add the `mutation` label to the PR to trigger targeted mutation testing. Trust-critical crates are:

- `shipper-core` (publish, reconcile, readiness, state)
- `shipper-encrypt`
- `shipper-output-sanitizer`
- `shipper-cargo-failure`
- `shipper-sparse-index`
- `shipper-registry`

## ripr Configuration

`ripr.toml` at the workspace root carries the schema ripr 0.5.0 expects:

```toml
[analysis]
mode = "draft"
include_unchanged_tests = true

[oracles]
snapshot_strength = "medium"
mock_expectation_strength = "medium"
broad_error_strength = "weak"

[severity.findings]
# … per-finding severity per `ripr init --root . --dry-run` ...

[severity.seams]
# … per-seam-shape severity per `ripr init --root . --dry-run` ...

[suppressions]
path = "policy/ripr-suppressions.toml"
```

Regenerate the canonical defaults at any time with `ripr init --root . --dry-run` and reconcile against the live file. The only deliberate divergence Shipper carries is the `[suppressions].path` override (Shipper keeps suppressions in `policy/` for ledger consistency rather than ripr's default `.ripr/suppressions.toml`).

## Suppression Format

```toml
[[suppression]]
finding_id = "ripr-2026-001"
path = "crates/shipper-core/src/engine/publish.rs"
owner = "engine-team"
reason = "Covered by BDD publish_resume.feature scenarios not visible to ripr."
created = "2026-05-12"
review_after = "2026-08-12"
```

`finding_id`, `path`, `owner`, and `reason` are required by ripr. `created` and `review_after` are Shipper conventions added so suppressions age out in line with the rest of `policy/`.

## Nightly Mutation Scope

Full mutation runs nightly against the trust-critical surface:

```
shipper-core
shipper-types
shipper-encrypt
shipper-output-sanitizer
shipper-cargo-failure
shipper-sparse-index
shipper-registry
shipper-cli
shipper
```

The mutation workflow uses `.cargo/mutants.toml` to set per-mutant and minimum test timeouts.

## xtask Integration

```bash
# Produce PR-scoped exposure evidence under target/ripr/pr/.
cargo xtask ripr-pr --base origin/main --head HEAD

# Verify the required PR evidence files are present and readable.
cargo xtask ripr-pr --check

# Produce line-placeable review guidance under target/ripr/review/.
cargo xtask ripr-review-comments --base origin/main --head HEAD

# Regenerate the public README badge endpoints.
cargo xtask badges

# Check committed public badge endpoint drift.
cargo xtask badges --check
```

`cargo xtask ripr-pr` is a thin wrapper that invokes `ripr check --root . --base <base> --format repo-exposure-json` (the `--head` flag is accepted for workflow symmetry; ripr 0.5 derives head from the working tree) after confirming the external `ripr` binary is on PATH. If `ripr` is not installed locally, the wrapper prints install instructions and exits success — local sessions are never blocked by a missing tool. CI installs a pinned version via `cargo install ripr --locked --version <pinned>` before the wrapper runs.

`cargo xtask ripr-review-comments` invokes `ripr review-comments` with explicit root/base/head/out arguments. It produces advisory review guidance only; it does not post inline comments or make merge decisions.

## Repo Badges

The public README `ripr+` badge is **repo-scoped**, not diff-scoped. Per upstream ripr policy, README badges should use repo-scoped seam-native evidence — a diff-scoped badge would read `0` on `main` simply because no diff exists, not because the repo is clean. PR-time artifacts remain diff-scoped and are never republished as README badges.

The committed Shields endpoint JSON is served via `raw.githubusercontent.com`:

| Path | Shields URL |
|---|---|
| `badges/ripr-plus.json` | `https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/EffortlessMetrics/shipper/main/badges/ripr-plus.json` |

The schema is the standard minimal Shields endpoint shape:

```json
{
  "schemaVersion": 1,
  "label": "ripr+",
  "message": "<count>",
  "color": "<color>"
}
```

Regenerate with:

```bash
cargo xtask badges
```

Check committed drift with:

```bash
cargo xtask badges --check
```

The command requires `ripr` on PATH (unlike `cargo xtask ripr-pr` which is advisory-only locally), honors `RIPR_BIN`, runs `ripr check --root <workspace> --format repo-badge-plus-shields`, validates the Shields shape, writes generated output under `target/xtask/badges/`, and copies only the public endpoint JSON into `badges/`. If the installed ripr release requires a precomputed `target/ripr/reports/test-efficiency.json` for the upstream plus projection, Shipper falls back to the repo-scoped `repo-exposure-json` count rather than using PR-scoped evidence. The legacy `cargo xtask repo-ripr-badge-artifacts` command remains as a compatibility alias for `cargo xtask badges`.

Refresh cadence is automated by `.github/workflows/badge-endpoints.yml`: the workflow regenerates the endpoint on pushes to `main` and opens a badge refresh pull request instead of pushing directly. The badge is a repo health endpoint, not a PR tax.
