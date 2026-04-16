# How to run a Shipper release in GitHub Actions

Goal: a tag push triggers a workspace release driven by Shipper. Interruption-safe, evidence-preserved.

> This repo dogfoods this setup — see `.github/workflows/release.yml` for the production example.

## Minimal workflow

```yaml
name: Release

on:
  push:
    tags: ['v*.*.*']

permissions:
  contents: write

jobs:
  publish:
    runs-on: ubuntu-latest
    environment: release
    timeout-minutes: 180
    steps:
      - uses: actions/checkout@v6

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install Shipper
        run: cargo install shipper-cli --locked

      - name: Plan
        run: |
          mkdir -p .shipper
          shipper plan --format json | tee .shipper/plan.txt

      - name: Upload plan artifact (before anything destructive)
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: shipper-state-plan
          path: .shipper/
          include-hidden-files: true
          retention-days: 30

      - name: Preflight
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: shipper preflight --policy safe

      - name: Publish
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: |
          shipper publish \
            --policy safe \
            --readiness-method both \
            --max-attempts 12 \
            --max-delay 15m

      - name: Upload final state (always)
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: shipper-state-final
          path: .shipper/
          include-hidden-files: true
          retention-days: 90
```

## Key considerations

### `include-hidden-files: true`

`.shipper/` is a hidden directory. Without this flag, the artifact upload silently skips it. This bit us in rc.1 (issue #89).

### Upload state at every stage

Upload the `.shipper/` directory after plan, after preflight, and after publish (or on failure). If the publish job times out or dies, the most recent artifact is what you need to resume.

### Timeout budget

For a first-publish release of many new crates, crates.io's 1-new-crate-per-10-min rate limit applies. Budget ~10 minutes per crate past the initial 5-crate burst. A 12-crate first publish can run 70–90 minutes. Set `timeout-minutes` accordingly (the example above uses 180).

### Token vs trusted publishing

The example uses `CARGO_REGISTRY_TOKEN`. For a safer posture, use Trusted Publishing (OIDC) — see [#96](https://github.com/EffortlessMetrics/shipper/issues/96) for the migration status. The short version:

```yaml
permissions:
  id-token: write
  contents: write

steps:
  - uses: rust-lang/crates-io-auth-action@v1  # exchanges OIDC for a short-lived token
  - run: shipper publish ...                   # uses the exchanged token automatically
```

### Resume mode

If a release is interrupted, manually trigger the resume workflow (a `workflow_dispatch` with `mode: resume` and `artifact_run_id: <failed run id>`) — or copy the resume job from this repo's `.github/workflows/release.yml`.

## Generate a template

```bash
shipper ci github-actions > .github/workflows/release.yml
```

This prints a recent-defaults template you can customize.

## See also

- [Tutorial: First publish](../tutorials/first-publish.md)
- [Tutorial: Recover from an interrupted release](../tutorials/recover-from-interruption.md)
- [Release runbook](../release-runbook.md) — operator reference for production releases
