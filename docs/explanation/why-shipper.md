# Why Shipper Exists

Cargo can already package and upload crates. Cargo 1.90 stabilized
`cargo publish --workspace` for multi-package releases. So why does Shipper
exist?

## The Short Answer

Because uploading is not the same as releasing reliably. The workflow around
`cargo publish` is where things break:

- Publishing is irreversible. You cannot delete a crates.io version; yank is
  containment, not undo.
- CI dies, networks partition, runners cancel, and rate limits exist.
- Some publish outcomes are ambiguous. Cargo's exit code can say "failed" while
  the upload actually succeeded.
- Operators need to trust the tool, which means knowing what it is doing live
  and reconciling what actually happened after the fact.

Shipper exists to own those responsibilities:

1. **Prove** - establish before the irreversible step that the release can
   succeed or explain what is not proven.
2. **Dispatch** - execute in a registry-aware, paced way.
3. **Reconcile** - close ambiguous outcomes against registry truth before
   retrying.
4. **Recover** - converge safely from durable state when interrupted.
5. **Remediate** - contain or fix-forward a partial release mechanically.

See [ROADMAP.md](../../ROADMAP.md) for status on each competency.

## What Shipper Does Not Do

Shipper is not a versioning or release-note orchestrator. It does not pick
version numbers, generate changelogs, create git tags, or author GitHub
Releases. Those are separate concerns with excellent existing tools
([cargo-release](https://github.com/crate-ci/cargo-release),
[release-plz](https://github.com/MarcoIeni/release-plz), and the `gh` CLI).

Shipper picks up after the version is decided, when the actual upload needs to
be safe, observable, recoverable, and auditable. That boundary keeps Shipper
narrow enough to be good at what it does.

## Why Finishability Has Three States

Preflight produces a `Finishability` enum: `Proven | NotProven | Failed`.
Three states are necessary because release proof is not binary.

- `Failed` means preflight found something actually wrong: dirty git, registry
  unreachable, dry-run failure, auth failure, or another blocking condition.
  Do not publish.
- `Proven` means every required check came back positive. Publish is expected
  to succeed within the selected proof tier.
- `NotProven` means checks ran without errors, but some could not complete to
  proven status. On a first publish of a brand-new crate, ownership and
  registry visibility cannot be verified because the crate does not exist yet.
  That is not a failure; it is an honest unknown.

Rehearsal registry support is how Shipper can eventually turn more
`NotProven` cases into `Proven` cases: publish to an alternate registry first,
wait for visibility, then verify install-from-registry resolution.

## Why Events Are Truth and State Is a Projection

Every state transition emits an event to `events.jsonl` before Shipper relies on
the resulting state. `state.json` is a derived snapshot that `shipper resume`
reads for fast recovery. `receipt.json` is an end-of-run summary.

When these three disagree, events win. The projection and summary are
conveniences; the append-only log is the ledger.

This matters because:

- If `state.json` corrupts or gets deleted, a run can be reconstructed from
  events alone.
- A tool consuming Shipper output should prefer events for correctness and state
  for speed.
- Consistency checks can detect drift between truth and projection, and any
  drift is a bug.

Full contract: [INVARIANTS.md](../INVARIANTS.md).

## Why the Engine Is a Library and the CLI Is an Adapter

Shipper has three product crates with distinct roles:

```text
shipper
  -> shipper-cli
       -> shipper-core
```

`shipper` is the install face and product-name facade. Users install it with
the `shipper` install facade; its binary forwards to `shipper_cli::run()`.
While public releases are prerelease-only, Cargo registry installs need an
explicit `--version`.
Its library re-exports a curated subset of `shipper-core` for callers that want
the product name without depending on every engine crate directly.

`shipper-cli` owns the terminal adapter: `clap` parsing, subcommand dispatch,
help text, progress rendering, snapshots, and human/JSON output.

`shipper-core` owns the release engine: plan, preflight, publish, resume,
registry profiles, reconciliation, state, events, receipts, and remediation
primitives. It has no CLI dependencies.

The practical reason: other frontends - IDP plugins (Backstage, Port, Cortex),
dashboards, custom automation, or agents - should be able to consume Shipper's
engine without shelling out and without pulling the terminal UX graph. The
library-first split makes that possible without a second rewrite.

The philosophical reason: publishing will grow more frontends: chat ops,
webhooks, status APIs, and release-control integrations. The engine is the
stable behavior surface; CLIs, plugins, and adapters come and go.

## Why We Forbid `unsafe`

`unsafe_code = "forbid"` is enforced workspace-wide. A tool whose pitch is
safety should not opt out of Rust's. There is no release justification for
`unsafe` blocks in orchestration code.

## Why This Is Not Just a Retry Wrapper

Retrying on failure is easy. The interesting questions are:

- Which failures should retry? (`ErrorClass::Retryable` vs `Permanent`)
- What about outcomes that might have succeeded despite a non-zero exit?
  (`ErrorClass::Ambiguous`)
- How do we know when it is safe to advance to a dependent crate? (Readiness
  verification against sparse index and API)
- What if the runner dies mid-retry? (Persisted state and plan-ID-guarded
  resume)
- What if a successful publish turns out to be broken? (Receipt-driven yank and
  fix-forward planning)

Each answer is a separate responsibility. Shipper owns them under one process,
with one durable evidence set. That is the thing Cargo is not, and should not
be.

## Cargo Stdout Is a Hint; the Registry Is the Truth

Cargo's `publish` command uploads to the registry, then polls the index, then
reports success or failure. The poll can time out while the upload succeeded.
Cargo's stdout/stderr are a human-facing log, not a stable machine protocol.

Shipper treats cargo text as a fast-path hint, never the authoritative answer:

- Classification into `ErrorClass::{Retryable, Permanent, Ambiguous}` comes
  from cargo output. Useful, but not definitive.
- On `Ambiguous`, Shipper never blind-retries. It polls the registry through
  the reconciliation flow and resolves one of `Published`, `NotPublished`, or
  `StillUnknown`.
- On `StillUnknown`, Shipper stops and surfaces the state for operator
  decision. Uploading a potential duplicate is worse than waiting.

This is why Shipper can be safer than a naive `cargo publish` loop in a shell
script: the shell script only has Cargo's exit code to go on, and Cargo's exit
code is sometimes the wrong source of truth.

## The Single Test

If we are doing our job, the single-sentence test from
[MISSION.md](../../MISSION.md) is true:

> You can start a release train, stop staring at the terminal, and still trust
> the outcome.

That is the product. Everything else is mechanism.
