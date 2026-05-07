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
- PRs labeled `coverage` or `full-ci`

Runs are conditional on the label check, so ordinary PRs do not trigger coverage.

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
