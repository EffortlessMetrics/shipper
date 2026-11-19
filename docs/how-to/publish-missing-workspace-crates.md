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

shipper doctor
shipper status
shipper preflight --policy safe
```

The unversioned install resolves the version exposed by the live public
registry; this source checkout alone does not prove that a candidate version
is published.

> **Destructive live-registry fence:** `shipper publish` can create permanent
> registry history. Run the next command only from an approved release commit,
> with final versions and explicit release authorization. A documentation or
> pull-request check stops after preflight or uses the safe mock rehearsal.

```bash
shipper publish --policy safe
```

If the run is interrupted, preserve `.shipper/` and classify its durable
evidence before choosing a command:

```bash
shipper status --durable
shipper inspect-events
shipper resume --policy safe
```

Run the final line only when the durable result says the evidence agrees and
resume is safe. `StillUnknown` and uncertain liveness do not authorize retry.

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

      - name: Diagnose local release prerequisites
        run: shipper doctor

      - name: Check registry state
        run: shipper status --format json

      - name: Preflight
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: shipper preflight --policy safe --format json

      - name: Publish missing package versions
        # Destructive: protect the `release` environment and require approval.
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
| General or permanent failure | `1` |
| Finalized partial result | `2` |
| Ambiguous cargo result reconciled to published | `0` |
| Ambiguous cargo result still unknown | `1` and no retry command |

For a completed run that finalized a receipt, use the
`outcome.next_action.kind` field in `shipper publish --format json` rather than
inferring recovery from the exit code alone. A completed receipt with retryable
unfinished work points to `resume`; a permanent failure points to
`resolve_blockers`; an uploaded package still awaiting visibility points to
`wait_for_registry`; unresolved ambiguous registry truth points to `reconcile`
and deliberately supplies no retry command. The human renderer reports the
same Result, rerun posture, next action, and retained evidence paths.

Plan-build failures and errors returned by the publish engine before a receipt
is finalized still exit 1. For `publish --format json`, those wrapped failures write a
`shipper.publish.error.v1` envelope to stderr and keep stdout empty. A null
safe-rerun value means no completed receipt proves whether a rerun is safe; fix
or investigate the reported category instead of blindly retrying.
If the engine persisted a `StillUnknown` reconciliation outcome before
stopping, the same error schema instead reports category `ambiguous`, an
explicitly unsafe rerun posture, a commandless `reconcile` action, and the
resolved state, event, and reconciliation evidence paths. It does not claim a
receipt or change the exit code from `1`.
Argument-parsing and other usage errors also exit 2, but use Clap's usage
output and create no `.shipper/` execution evidence. That is distinct from a
finalized partial-result exit 2. Configuration and option-validation failures outside the wrapped
plan/engine boundary retain their existing output. Early-error JSON is separate
from `shipper.publish.v1`'s completed-receipt outcome.

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
