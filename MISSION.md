# Mission, Vision, and What We Believe

This document is Shipper's north star. When the [roadmap](ROADMAP.md) ages or specific issues drift, this is what gets re-read. Everything else descends from here.

## Mission

Shipper makes publishing Rust crates to a registry **safe to start and safe to re-run**. We encode what Cargo doesn't: pre-flight proof, ambiguity reconciliation, mechanical recovery, and operator-grade observability — so a multi-crate workspace publish becomes a boring, auditable operation instead of a manual coordination dance.

## Vision

Five years from now, every Rust workspace maintainer treats `shipper publish` the way they treat `cargo test` — boring, trustworthy, and unsurprising. When releases go wrong, the tool that planned them is the tool that contains them. Trusted Publishing is the default; long-lived tokens are the exception. CI is publishing's natural environment, not its hostile one.

## Audience

Shipper is for workspace maintainers who:

- Publish **multiple interdependent crates** as a coherent release
- Treat publishing as a **serious, audited operation**
- Run releases **through CI**, not from a developer laptop
- Need to **recover correctly** when things go wrong, not heroically

We are explicitly not optimizing for one-shot toy publishes or solo single-crate authors who are well-served by `cargo publish` directly. Their workflow is fine. Ours is what happens when the workflow has consequences.

## What we believe

These convictions produce every design decision. When in doubt, return here.

### 1. Cargo is excellent at packaging and uploading. The workflow around it is where reliability breaks.
Shipper does not replace cargo. It wraps the operations cargo treats as one-shot into something resumable, observable, and recoverable.

### 2. Publishing is irreversible.
Once a version is on crates.io, you cannot delete it (yank is containment, not undo). This asymmetry should shape every default. Defaults err toward verifying, logging, and providing evidence — not toward speed. Faster paths are explicit opt-ins.

### 3. CI dies. Networks partition. Runners cancel. Rate limits exist.
A publishing tool that pretends these don't happen is one that loses data the first time they do. State persists after every step. Resume works without operator heroics.

### 4. Registries can lie about state.
Cargo's upload-then-poll model means the command can fail while the upload succeeded. Stdout is a hint; the registry itself is the truth. Ambiguity is reconciled against the registry, never blind-retried.

### 5. Operators must trust the tool.
Trust comes from **legibility** ("what is it doing right now?") and **reconciliation** ("what actually happened?"). Silent retry loops corrode both. Receipts and evidence are how we prove what happened, after the fact, to anyone — including ourselves.

### 6. The engine is a library; the CLI is a frontend.
Both exist. Neither contains the other. The library does not depend on the CLI. Other frontends — IDP plugins, dashboards, automation — consume the library directly.

### 7. Events are truth. State is projection.
`events.jsonl` is the authoritative record of what happened. `state.json` is a derived projection over events for resume convenience. `receipt.json` is a summary at end-of-run. When they disagree, events win. See [docs/INVARIANTS.md](docs/INVARIANTS.md).

### 8. Determinism is a feature.
The same workspace state always produces the same `plan_id`. Plans are reproducible across environments and time. This is the foundation that makes resume safe and audit possible.

### 9. We forbid `unsafe`.
`unsafe_code = "forbid"` is enforced workspace-wide. A tool whose pitch is safety should not opt out of Rust's.

## What we are not

Shipper is not:

| Not this | Use instead |
|---|---|
| A release orchestrator | [cargo-release](https://github.com/crate-ci/cargo-release), [release-plz](https://github.com/MarcoIeni/release-plz) |
| A version-decision tool | cargo-release / release-plz |
| A changelog generator | [git-cliff](https://github.com/orhun/git-cliff), release-plz |
| A git-tag / GitHub-release creator | `gh` CLI, GitHub Actions |
| A multi-ecosystem publishing tool | Specialized tools per ecosystem |
| A general-purpose CI framework | GitHub Actions, GitLab CI, etc. |

Rust → crates.io is the focus. Alternative cargo-protocol registries (Cloudsmith, kellnr, self-hosted) are supported as they appear; other ecosystems are out of scope.

## How we measure success

The product thesis — the **nine competencies** in [ROADMAP.md](ROADMAP.md) — is the structured measure. Each competency is graded Done / Partial / Missing against engine behavior. We close gaps in the order set by ROADMAP's *Now / Next / Later*.

A short definition of *good enough for v0.3.0 stable*:

> When an operator can push a tag, walk away for an hour, come back to a complete release with evidence, and have the tool recover automatically when things go wrong — without external monitoring — Shipper has earned the trust its mission claims.

The longer definition lives in [ROADMAP.md](ROADMAP.md) under *Now*.

## How to read this repo

| If you are… | Start here |
|---|---|
| A new user | [README.md](README.md) → install, quick start, examples |
| An operator setting up CI | [docs/release-runbook.md](docs/release-runbook.md), [docs/configuration.md](docs/configuration.md) |
| A contributor | This document → [ROADMAP.md](ROADMAP.md) → [CONTRIBUTING.md](CONTRIBUTING.md) → an issue from #100–#109 |
| Auditing receipts or events | [docs/INVARIANTS.md](docs/INVARIANTS.md) |
| An AI assistant | [CLAUDE.md](CLAUDE.md) or [GEMINI.md](GEMINI.md), then this document |
