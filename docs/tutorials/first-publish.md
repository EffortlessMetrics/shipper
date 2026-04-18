# Tutorial: First publish from a toy workspace

In this tutorial you will publish a two-crate workspace to crates.io using Shipper. By the end you'll have run the full `plan → preflight → publish → inspect` flow and seen what `.shipper/` state files look like after a successful release.

## What you'll learn

- Install Shipper and initialize a config
- Read a plan (what will be published, in what order)
- Run a preflight check (what could go wrong, safely)
- Execute the publish
- Inspect the receipt

## What you'll need

- Rust toolchain (edition 2024, MSRV 1.92 — `rustup update stable` is fine)
- A crates.io account and a token (`cargo login`)
- A git-clean workspace with two crates, one depending on the other
- About 15 minutes

> If you want a throwaway target, publish `0.0.1-dev.1` of a unique-name crate you control. This tutorial assumes you are publishing *your own* crates — Shipper will not invent version numbers.

## 1. Install

```bash
cargo install shipper --locked
```

> The binary is named `shipper`; the published crate is `shipper-cli`. [Issue #95](https://github.com/EffortlessMetrics/shipper/issues/95) tracks making `cargo install shipper` work.

Confirm:

```bash
shipper --version
```

## 2. Create a config

```bash
cd /path/to/your/workspace
shipper config init
```

This writes `.shipper.toml` with sensible defaults. Open it; the defaults are fine for a first pass. Important defaults:

- `policy = "safe"` — verify every step
- `readiness = { method = "api", ... }` — check crates.io API before advancing to dependents

## 3. Plan the release

```bash
shipper plan
```

You'll see a dependency-ordered list of crates and a `plan_id` (SHA256). Two properties to notice:

- The order respects your dependency graph (a dependency is published before its dependents).
- The `plan_id` is deterministic: run `shipper plan` again and the ID is identical. If it changes, your workspace state changed — that's your early-warning for "something is different than you think."

## 4. Preflight

```bash
shipper preflight
```

Preflight runs checks without publishing:

- Git working tree must be clean (or use `--allow-dirty`)
- Registry must be reachable
- `cargo publish --dry-run` must succeed for the workspace
- For each crate: version must not already exist on crates.io
- Optionally: ownership is verified (requires a token)

Output ends with a `finishability` value:

- `Proven` — everything checks out
- `NotProven` — some checks could not be completed (e.g., ownership checks fail for crates that don't exist yet — that's normal for a first publish)
- `Failed` — something is actually wrong; read the error above

> `NotProven` is not the same as `Failed`. First publishes of brand-new crates are inherently not-provable (you can't verify ownership of a crate that doesn't exist). See [explanation/why-shipper.md](../explanation/why-shipper.md#why-finishability-has-three-states) for why this distinction exists.

## 5. Publish

```bash
shipper publish
```

Shipper will:

1. Publish each crate via `cargo publish`, in dependency order
2. After each publish, wait for the new version to be visible on crates.io before moving to dependents
3. Retry with backoff on transient failures (HTTP 429, network blips)
4. Persist state to `.shipper/` after every step

Expected wall-clock for a 2-crate workspace: 1–3 minutes.

If something goes wrong mid-publish, see [the recovery tutorial](recover-from-interruption.md).

## 6. Inspect what happened

```bash
shipper inspect-receipt
```

Human-readable summary of what was published, with timestamps.

```bash
shipper inspect-events
```

The full event log — every state transition with timestamp.

```bash
ls .shipper/
# events.jsonl   <- append-only truth
# state.json     <- projection for resume
# receipt.json   <- end-of-run summary
```

See [explanation/INVARIANTS.md](../INVARIANTS.md) for how these three files relate.

## 7. What's next

- If you plan to publish from CI, follow [How-to: run in GitHub Actions](../how-to/run-in-github-actions.md).
- To understand Shipper's safety model, read [Why Shipper exists](../explanation/why-shipper.md).
- For the full command/flag reference, run `shipper --help` or see [reference/cli.md](../reference/cli.md).
