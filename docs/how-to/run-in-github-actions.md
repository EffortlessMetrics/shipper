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
        run: cargo install shipper --locked

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

### Token vs Trusted Publishing

The example above uses `CARGO_REGISTRY_TOKEN` — a long-lived personal
access token stored as a repo secret. **Prefer Trusted Publishing
(OIDC)**: short-lived tokens, scoped to a specific repo + workflow + ref
pattern + GitHub Actions environment. No secrets to rotate, no PATs to
leak.

**One-time setup on crates.io** (per crate):

1. Log in to <https://crates.io>, open the crate's **Settings →
   Trusted Publishing** panel.
2. Add a new trusted publisher:
   - Repository: `<owner>/<repo>`
   - Workflow filename: `release.yml` (or whatever yours is called)
   - Environment: `release` (match the `environment:` name in the job
     below — this is the scope guard)
3. Repeat for **every** crate the workspace publishes. Do NOT enable
   OIDC until the list is complete.

> **Why "every crate"**: if only some crates are registered, the
> OIDC action still succeeds and mints a token — but that token 401s
> on the unregistered crates mid-train, after some publishes have
> already succeeded. Shipper's preflight catches scope mismatches
> for *existing* crates via ownership checks, but new crates have no
> owner record yet so the first-publish case depends on operator
> discipline. Complete registration first; rehearse second; tag third.
>
> **Rehearsal validates the mechanism.** `release.yml`'s
> `release-rehearse` job binds to `environment: release` so the OIDC
> scope it mints matches production. A rehearsal that mints
> successfully proves the scope wiring. A mid-train 401 on a
> different crate proves you missed a registration step — fix the
> missing registration, don't retry the tag.

**Workflow**:

```yaml
permissions:
  contents: write
  id-token: write           # required to mint the OIDC token

jobs:
  publish:
    runs-on: ubuntu-latest
    environment: release    # must match the crates.io trusted-publisher config
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install shipper --locked

      # Exchange the workflow's OIDC token for a short-lived
      # crates.io publish token. Output: steps.auth.outputs.token.
      - id: auth
        uses: rust-lang/crates-io-auth-action@v1

      - name: shipper publish
        env:
          # Falls back to the long-lived secret if OIDC is unavailable
          # (e.g. during incident response or the first bootstrap run).
          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token || secrets.CARGO_REGISTRY_TOKEN }}
        run: shipper publish --policy safe
```

**Troubleshooting**:

- `id-token: write` missing → GitHub refuses the OIDC exchange → the
  action fails loudly; add the permission.
- Crate not registered as a trusted publisher → `cargo publish` returns
  401 despite a valid-looking token. Check crates.io's Trusted
  Publishing panel for the crate.
- Tag/branch mismatch → token minted for the wrong ref pattern →
  crates.io refuses. The `environment:` name is the tightest scope —
  make sure the workflow's environment matches what you registered.

See `.github/workflows/release.yml` in this repo for the production
example, and [#96](https://github.com/EffortlessMetrics/shipper/issues/96)
for the migration history.

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
