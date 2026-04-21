# AGENTS.md

## Purpose

This directory owns the user-facing docs set: tutorials, how-to guides, reference docs, and explanation docs.

## Key files

- `README.md` — docs index and Diataxis map.
- `reference/cli.md` — canonical CLI reference.
- `INVARIANTS.md` — events/state/receipt authority rules.
- `release-runbook.md` and `how-to/` — operator-facing workflows.

## Invariants

- Keep docs aligned with the install-facing command name: `shipper`.
- Preserve the Diataxis split: tutorials teach, how-to guides solve tasks, reference docs specify, explanation docs justify.
- When describing `.shipper/` artifacts, keep the authority order consistent with `INVARIANTS.md`.

## Checks

- Update `docs/README.md` when adding or moving docs.
- Spot-check commands, file names, and flags against the current CLI help or tests.
