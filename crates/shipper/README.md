# shipper

Installable Shipper facade for Rust workspaces.

Shipper publishes missing workspace crate versions in dependency order,
verifies registry visibility before advancing, and retains the evidence needed
to explain or resume a partial run. It assumes versions, changelogs, and tags
are already chosen.

## Install

The supported install package is the `shipper` facade. This command resolves
the latest version exposed by the public registry when it runs:

```bash
cargo install shipper --locked
```

The retained public-install evidence available when this source snapshot was
prepared covers 0.4.0. Pin that verified baseline for reproducibility:

```bash
cargo install shipper --version 0.4.0 --locked
```

This snapshot was prepared for 0.5.0 before its public result existed. A source
version or README cannot prove registry publication; check the live registry
and release evidence for the version available now.

From a checkout, exercise the same facade with:

```bash
cargo run -p shipper -- --help
```

## First useful path

```bash
shipper doctor      # diagnose workspace, auth, and registry posture
shipper plan        # preview the dependency-ordered graph
shipper preflight   # assess readiness as PROVEN / NOT PROVEN / FAILED
shipper publish     # publish missing versions and retain evidence

shipper status          # registry-aware status
shipper inspect-events  # chronological event detail
shipper inspect-receipt # summarized retained evidence
shipper resume          # continue from agreeing retained evidence
```

The 0.5.0 candidate also adds `shipper status --durable`, a local,
registry-bypassing view that refuses to call a run resumable when evidence or
liveness is inconclusive. Use `shipper --help` and
`shipper <command> --help` as the canonical command surface.

## Facade ownership

`shipper` is the user-facing package: the binary most operators install and a
curated product-name library surface. It wraps:

- a small binary that forwards to `shipper_cli::run()`;
- curated re-exports of engine modules for product-name embedders;
- install-facing documentation.

The sibling crates own the implementation seams:

- [`shipper-cli`](https://crates.io/crates/shipper-cli) owns Clap parsing,
  command dispatch, rendering, and exit behavior;
- [`shipper-core`](https://crates.io/crates/shipper-core) owns the reusable
  engine without CLI dependencies.

Most library consumers should use `shipper-core` or the curated `shipper`
re-exports. Direct `shipper-cli` use is for specialized command embedding.

## Evidence contract

```text
events.jsonl         = authoritative truth
state.json          = resumable projection
receipt.json        = derived summary
reconciliation.json = registry-truth evidence for ambiguous publish outcomes
```

When these disagree, stop and investigate rather than retrying blindly. A
`StillUnknown` report is the evidence to inspect before recovery.

## Scope and support

Shipper publishes, reconciles, and resumes. It does not choose versions,
generate changelogs, tag releases, or create GitHub Releases.

- [Project README](https://github.com/EffortlessMetrics/shipper#readme)
- [Documentation](https://github.com/EffortlessMetrics/shipper/tree/main/docs)
- [Support tiers](https://github.com/EffortlessMetrics/shipper/blob/main/docs/status/SUPPORT_TIERS.md)
- [0.5.0 migration guide](https://github.com/EffortlessMetrics/shipper/blob/v0.5.0/docs/release/0.5.0-migration.md) (resolves only after release authority creates that tag)

## Stability

Pre-1.0. Breaking changes are called out in the
[`CHANGELOG.md`](https://github.com/EffortlessMetrics/shipper/blob/main/CHANGELOG.md).

## License

MIT OR Apache-2.0.
