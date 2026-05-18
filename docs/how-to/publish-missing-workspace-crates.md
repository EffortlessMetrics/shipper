# Publish missing workspace crates

Use this when you already decided versions and want CI to publish only missing
workspace package versions.

## What this does

- Skips `name@version` pairs that already exist on the registry.
- Publishes missing versions in dependency order.
- Fails non-zero on real/unsafe outcomes.
- Leaves `.shipper/` evidence for audit and resume.

## Quick local sequence

```bash
cargo install shipper --locked

shipper status
shipper preflight --policy safe
shipper publish --policy safe
```

If the run is interrupted, rerun with:

```bash
shipper resume --policy safe
```

## Minimal GitHub Actions recipe

```yaml
name: Publish missing workspace crates

on:
  workflow_dispatch:

jobs:
  publish:
    runs-on: ubuntu-latest
    environment: release
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable

      - name: Install Shipper
        run: cargo install shipper --locked

      - name: Check registry state
        run: shipper status --format json

      - name: Preflight
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: shipper preflight --policy safe --format json

      - name: Publish missing package versions
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: shipper publish --policy safe --format json

      - name: Upload Shipper evidence
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: shipper-evidence
          path: .shipper/
          include-hidden-files: true
```

## Exit behavior contract

| Scenario | Exit |
|---|---:|
| All versions already exist | `0` |
| Mixed existing and missing, publish succeeds | `0` |
| Permanent publish failure | non-zero |
| Ambiguous cargo result reconciled to published | `0` |
| Ambiguous cargo result still unknown | non-zero |

## Important boundary

Shipper publishes missing **versions**, not changed sources.

If `foo@1.2.3` already exists, Shipper skips it even if local code changed.
Bump the version first, then rerun publish.

## See also

- [How to run a Shipper release in GitHub Actions](run-in-github-actions.md)
- [Tutorial: Recover from an interrupted release](../tutorials/recover-from-interruption.md)
- [CLI reference](../reference/cli.md)
- [Support tiers](../status/SUPPORT_TIERS.md)
- [SHIPPER-SPEC-0007](../specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md)
