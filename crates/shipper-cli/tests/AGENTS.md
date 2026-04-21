# AGENTS.md

## Purpose

This test tree covers the install-facing CLI behavior: parsing, help text, snapshots, and end-to-end command flows.

## Key files

- `cli_snapshots.rs` — help and error output snapshots.
- `e2e_expanded.rs` — broad CLI snapshot coverage.
- `bdd_*.rs` — operator-facing workflow tests.
- `snapshots/` — accepted CLI output fixtures.

## Invariants

- The product-facing command name is `shipper`.
- Snapshot updates should follow intentional output changes, not test convenience.
- Keep environment-dependent output normalized so snapshots stay portable across platforms.

## Checks

- Run the smallest affected test target first, for example `cargo test -p shipper-cli --test cli_snapshots` or `--test e2e_expanded`.
- Review snapshot diffs before finishing any CLI-output change.
