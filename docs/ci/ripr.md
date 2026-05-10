# ripr — Reachable Incremental PR Coverage

This document describes the `ripr` advisory lane for `shipper`: what it does, when it runs, what it means, and how to act on its findings.

## What ripr Does

`ripr` is a PR-time exposure filter. Given the diff of a PR, it identifies which mutants are reachable from the changed code and reports which of those reachable mutants are not covered by any test. The result is a ranked list of test-coverage gaps that are immediately relevant to the PR.

`ripr` is not a replacement for mutation testing. It does not generate and kill mutants. It answers a narrower question: "For the code this PR touches, where do tests not reach at all?"

Full mutation testing answers: "Could a subtle semantic change survive the entire test suite?" That belongs in targeted, nightly, and release lanes.

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

The report lists findings ranked by severity:

| Severity | Meaning |
|---|---|
| `severe` | Changed code calls into a critical trust path (publish, reconcile, encrypt, state) that has no test coverage. |
| `moderate` | Changed code calls into a path with partial coverage gaps. |
| `low` | Reachable code gap in lower-risk utility paths. |
| `informational` | Coverage gap exists but the path is already receipted in the suppression list. |

## When to Act on ripr Findings

**Act on severe findings:** A `severe` finding means the PR changes code that directly exercises a critical trust path with no test. Add a test or add a suppression with justification.

**Consider acting on moderate findings:** If the gap is small and a test is cheap to write, write it. If the path is covered by BDD or integration tests not visible to ripr, note this in the suppression receipt.

**Ignore informational findings:** These are already receipted.

## Triggering Full Mutation

If `ripr` reports a `severe` finding in a trust-critical crate, consider adding the `mutation` label to the PR to trigger targeted mutation testing. Trust-critical crates are:

- `shipper-core` (publish, reconcile, readiness, state)
- `shipper-encrypt`
- `shipper-output-sanitizer`
- `shipper-cargo-failure`
- `shipper-sparse-index`
- `shipper-registry`

## ripr Configuration

The `ripr.toml` file at the workspace root configures:

```toml
[targets]
# Crates included in ripr analysis.
include = [
  "shipper-core",
  "shipper-cli",
  "shipper-config",
  "shipper-types",
  "shipper-duration",
  "shipper-retry",
  "shipper-encrypt",
  "shipper-registry",
  "shipper-sparse-index",
  "shipper-cargo-failure",
  "shipper-webhook",
  "shipper-output-sanitizer",
]

[severity]
# Crates where reachable gaps are treated as severe.
trust_critical = [
  "shipper-core",
  "shipper-encrypt",
  "shipper-output-sanitizer",
  "shipper-cargo-failure",
  "shipper-sparse-index",
  "shipper-registry",
]
```

## Suppression Format

```toml
[[suppression]]
path = "crates/shipper-core/src/engine/publish.rs"
finding_id = "ripr-2026-001"
reason = "Covered by BDD publish_resume.feature scenarios not visible to ripr."
owner = "team"
```

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
# Run ripr analysis for the current PR diff.
cargo xtask ripr-pr

# Run ripr with a specific base.
cargo xtask ripr-pr --base origin/main

# Dry-run targeted mutation for changed files.
cargo xtask mutants-pr --changed --dry-run

# Run targeted mutation for changed files.
cargo xtask mutants-pr --changed
```
