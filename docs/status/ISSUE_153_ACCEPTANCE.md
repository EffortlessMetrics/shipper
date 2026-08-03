# Issue #153 Acceptance Ledger

The canonical, current ledger is
[`plans/0.5.0-scheduler-conformance.md`](../../plans/0.5.0-scheduler-conformance.md).
This status page remains as a compatibility pointer so older links do not
reintroduce the obsolete split between sequential package execution and
`engine/parallel/publish.rs`.

Implementation evidence is merged through PR #234. The issue remains open
until exact-head routed Rust/CLI proof and retained interruption/release
evidence are recorded; this page is not release authorization.

## Current proof commands

```text
cargo fmt --all -- --check
cargo test -p shipper-core --lib mode_parity_corpus_sequential_matches_parallel --locked
cargo nextest run -p shipper-cli --test bdd_parallel --locked --profile ci
cargo nextest run -p shipper-cli --test e2e_rehearse --locked --profile ci
cargo clippy -p shipper-core --all-targets --all-features --locked -- -D warnings
git diff --check
```

The plan ledger distinguishes cross-scheduler cases from the parallel-only
worker/join contract and from separately routed interruption rehearsal. New
0.5 artifacts must rebuild to their claimed state; 0.4 artifacts remain
readable and resumable, with fields absent from the old vocabulary treated as
unknown.

## Closeout boundary

Do not close #153 until the final exact-head evidence bundle links every plan
row to a test or retained artifact, proves the complete routed gate, and
records the remaining CLI/release-authority boundaries. No public executor
type, new crate, release tag, publication, or credential movement is part of
this issue.
