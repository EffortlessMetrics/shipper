# Coverage

Codecov coverage is execution-surface evidence. It answers:

> Did tests execute this Rust surface?

## What coverage does not answer

- Whether publish execution is correct
- Whether registry visibility reconciliation is correct
- Whether ambiguous `cargo publish` recovery is correct
- Whether token redaction is safe
- Whether encrypted state handling is safe
- Whether full-strength crypto property tests are adequate
- Whether fuzzing is sufficient
- Whether release readiness is proven

Those are separate proof lanes, tracked independently.

## Workflow triggers

The Coverage workflow runs on:

- Push to `main`
- `workflow_dispatch` (manual trigger)
- Code-changing PR events where `coverage` or `full-ci` is already present

Runs are conditional on the label check, so ordinary PRs do not trigger
coverage. Applying a label alone deliberately does not emit a coverage run or
cancel the authoritative code-change run. The next opened, synchronize, or
reopened event evaluates the label. For an immediate refresh, dispatch the
workflow independently; `workflow_dispatch` is unconditional and does not
consume the label.

Canonical label metadata lives in `policy/ci-trigger-labels.toml`. Maintainers
can run `cargo xtask ci-labels check` for offline source agreement and
`cargo xtask ci-labels check-live --repo EffortlessMetrics/shipper-swarm` for a
read-only live drift check. Live creation/update requires the explicit
`cargo xtask ci-labels sync --repo EffortlessMetrics/shipper-swarm --apply`
path.

## Durable receipts

Coverage evidence persists in:

- `coverage.json` — machine-readable coverage data
- `coverage.txt` — human-readable summary
- `lcov.info` — LCOV format for external tools
- GitHub Actions coverage artifact (14-day retention)
- Codecov dashboard

## Configuration

Coverage is configured via:

- `.github/workflows/coverage.yml` — workflow definition
- `codecov.yml` — Codecov status and reporting settings (advisory, not blocking)

## Safety boundary

Coverage statements apply only to:

- Code paths exercised by the test suite
- Under the current instrumentation configuration (PROPTEST_CASES=16 for cost control)

Claims do not extend to:

- Untested code paths
- Theoretical correctness of the shipper publishing pipeline
- Safety guarantees about registry state reconciliation
- Correctness of token handling or encrypted state
- Readiness for production release
