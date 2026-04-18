# shipper

Reliable, resumable `cargo publish` for Rust workspaces.

```text
cargo install shipper --locked
```

Shipper runs a multi-crate workspace release to crates.io (or any
Cargo-compatible registry) with the safety guarantees that `cargo
publish` alone can't give you:

- **Resumable** — if a publish is interrupted (CI timeout, rate limit,
  network blip), `shipper resume` picks up from exactly where it
  stopped. Already-published crates are skipped; ambiguous crates
  reconcile against the registry first.
- **Backoff-aware** — 429s and transient network errors retry with
  jittered exponential backoff. Permanent failures fail fast.
- **Events-as-truth** — every step writes to `.shipper/events.jsonl`.
  `state.json` is a projection, `receipt.json` is a summary. When the
  three disagree, events win.
- **Prove before publish** — optional rehearsal against an alternate
  registry (`shipper rehearse`) that packages, verifies, and
  install-smokes every crate before touching crates.io.
- **Contain damage** — receipt-driven `shipper yank`, reverse-topological
  yank plans, and fix-forward planning for partial or compromised
  releases.
- **Trusted Publishing** — OIDC authentication against crates.io in CI
  via GitHub's `rust-lang/crates-io-auth-action`, no long-lived tokens
  required.

## Architecture

```text
shipper (this crate — install face)
  -> shipper-cli (CLI adapter: clap parsing, dispatch, output)
       -> shipper-core (engine: plan, preflight, publish, resume, …)
```

Three crates, one product. You install `shipper`. If you're embedding
the engine in your own Rust tool, depend on
[`shipper-core`](https://crates.io/crates/shipper-core) directly — it
has no CLI dependencies (no `clap`, no `indicatif`).

## Quick start

```bash
# In a Rust workspace with crates you want to publish
cargo install shipper --locked

# Preview the plan + preflight
shipper preflight

# Publish (writes receipt, events, state to .shipper/)
shipper publish

# If interrupted, continue from where it stopped
shipper resume
```

See [the how-to guides](https://github.com/EffortlessMetrics/shipper/tree/main/docs/how-to)
for rehearsal against an alternate registry, remediating a compromised
release, and running recovery drills.

## Scope

Shipper **does** handle publishing, retrying, resuming, rehearsing,
yanking, and fix-forward planning. It **does not** decide version
numbers, generate changelogs, tag releases, or create GitHub
releases — pair it with your preferred versioning/release workflow.

## Stability

Pre-1.0. Breaking changes are called out in
[`CHANGELOG.md`](https://github.com/EffortlessMetrics/shipper/blob/main/CHANGELOG.md).

## License

MIT OR Apache-2.0.
