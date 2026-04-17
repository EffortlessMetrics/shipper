# Why Shipper exists

Cargo can already package and upload crates. Cargo 1.90 even stabilized `cargo publish --workspace` for multi-package releases. So why does Shipper exist?

## The short answer

Because *uploading* is not the same as *releasing reliably*. The workflow around `cargo publish` is where things break:

- Publishing is **irreversible** (you cannot delete a crates.io version; yank is containment, not undo)
- CI dies, networks partition, runners cancel, rate limits exist
- Some publish outcomes are **ambiguous** — cargo's exit code can say "failed" while the upload actually succeeded
- Operators need to *trust* the tool, which means knowing what it's doing live and reconciling what actually happened after the fact

Shipper exists to own those five responsibilities, so publishing becomes boring:

1. **Prove** — establish before the irreversible step that the release can succeed
2. **Dispatch** — execute in a registry-aware, paced way
3. **Reconcile** — close ambiguous outcomes against registry truth before retrying
4. **Recover** — converge safely from durable state when interrupted
5. **Remediate** — contain or fix-forward a partial release mechanically

See [ROADMAP.md](../../ROADMAP.md) for status on each.

## What Shipper does *not* do

Shipper is not a release orchestrator. It doesn't pick version numbers, generate changelogs, create git tags, or author GitHub Releases. Those are separate concerns with excellent existing tools ([cargo-release](https://github.com/crate-ci/cargo-release), [release-plz](https://github.com/MarcoIeni/release-plz), `gh` CLI).

Shipper picks up **after** the version is decided and the tag is cut, when the actual upload needs to be safe and recoverable. That boundary matters — it's what keeps Shipper narrow enough to be good at what it does.

## Why finishability has three states

Preflight produces a `Finishability` enum: `Proven | NotProven | Failed`. Three states, not two.

- **`Failed`** — preflight found something actually wrong (git dirty, registry unreachable, dry-run fails). Don't publish.
- **`Proven`** — every check came back positive. Publish is expected to succeed.
- **`NotProven`** — checks ran without errors, but some couldn't complete to "proven" status. Example: on a first-publish of a brand-new crate, ownership cannot be verified because the crate doesn't exist yet. That's not a failure; it's epistemically honest.

`NotProven` reads scary but is the correct answer in common situations. [Rehearsal registry (#97)](https://github.com/EffortlessMetrics/shipper/issues/97) is how we eventually turn many `NotProven` cases into `Proven` ones — by publishing to an alternate registry first and verifying install-from-registry resolution.

## Why events are truth and state is a projection

Every state transition emits exactly one event to `events.jsonl` (append-only). `state.json` is a derived snapshot that `shipper resume` reads for fast recovery. `receipt.json` is an end-of-run summary.

When these three disagree, events win. The projection and summary are conveniences; the append-only log is the ledger.

This matters because:

- If `state.json` corrupts or gets deleted, the run can be reconstructed from events alone.
- A tool consuming Shipper output should prefer events for correctness and state for speed.
- We can add consistency checks that detect drift between truth and projection — and any drift is a bug.

Full contract: [INVARIANTS.md](../INVARIANTS.md).

## Why the engine is a library and the CLI is thin

`crates/shipper` contains all the domain logic. `crates/shipper-cli` parses args and calls into the library.

The practical reason: other frontends — IDP plugins (Backstage, Port, Cortex), dashboards, custom automation — should be able to consume Shipper directly without shelling out. The library-first split makes that possible without a second rewrite.

The philosophical reason: publishing is going to grow more frontends (chat ops, webhooks, status APIs, etc.). The library is the stable surface; CLIs, plugins, and adapters come and go.

## Why we forbid `unsafe`

`unsafe_code = "forbid"` is enforced workspace-wide. A tool whose pitch is safety should not opt out of Rust's. There is no release for `unsafe` blocks in orchestration code.

## Why this isn't "just a retry wrapper"

Retrying on failure is easy. The interesting questions are:

- *Which* failures should retry? (`ErrorClass::Retryable` vs `Permanent`)
- What about outcomes that might have succeeded despite a non-zero exit? (`ErrorClass::Ambiguous` — and this is where registry reconciliation matters)
- How do we know when it's safe to advance to a dependent crate? (Readiness verification against sparse index + API)
- What if the runner dies mid-retry? (Persisted state + plan-ID-guarded resume)
- What if a successful publish turns out to be broken? (Receipt-driven yank / fix-forward — planned)

Each answer is a separate responsibility. Shipper owns them all, under a single process, with a single durable ledger. That's the thing Cargo is not, and shouldn't be.

## Cargo stdout is a hint; the registry is the truth

Cargo's `publish` command uploads to the registry, then polls the index, then reports success or failure. The poll can time out while the upload succeeded. Cargo's stdout/stderr are a human-facing log — explicitly **not** a stable machine protocol.

Shipper treats cargo text as a **fast-path hint**, never the authoritative answer:

- Classification into `ErrorClass::{Retryable, Permanent, Ambiguous}` comes from pattern-matching cargo's stderr. Useful. Not definitive.
- On `Ambiguous` (cargo exit uncertain), Shipper **never blind-retries**. It polls the registry (sparse index + API) via the reconciliation flow and resolves one of `Published` / `NotPublished` / `StillUnknown`. The registry is authoritative.
- On `StillUnknown` (even the registry queries couldn't resolve), Shipper halts and surfaces the state for operator decision — uploading a potential duplicate is worse than waiting.

This is why Shipper can be *safer* than a naive `cargo publish` loop in a shell script: the shell script only has cargo's exit code to go on, and Cargo's exit code is sometimes just wrong about whether the upload happened.

## The single test

If we're doing our job, the single-sentence test from [MISSION.md](../../MISSION.md) is true:

> You can start a release train, stop staring at the terminal, and still trust the outcome.

That's the product. Everything else is mechanism.
