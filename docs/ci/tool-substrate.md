# Tool Substrate Standard

Shipper standardizes on a small upstream substrate, then exposes repo-shaped
commands through `cargo xtask`. Upstream tools are the engine room; `xtask` is
Shipper's public control surface for contributors, agents, and CI.

## Doctrine

```text
Do not make upstream tools the repo's public control surface.
Make xtask the repo surface.
Make upstream tools the engine room.
```

A lane may still document the upstream command for debugging, but CI policy and
agent instructions should prefer the stable wrapper. This keeps policy encoded in
one Rust entry point instead of scattering it across workflow YAML, shell
fragments, and tool-specific flags.

## Core substrate

| Plane | Upstream substrate | Repo-facing role |
|---|---|---|
| Repository orchestration | `xtask` | Stable command surface for checks, reports, and routed evidence lanes. |
| Source exception ledgers | `cargo-allow`-style ledgers plus Shipper `policy/*.toml` ledgers | Receipt intentional exceptions with owner, reason, creation date, and review date. |
| Syntax and codemods | `ast-grep`; rust-analyzer crates when Rust identity must be durable | Find syntactic candidates quickly; let Rust-aware tooling decide authoritative Rust policy. |
| Workspace graph | `cargo_metadata`; `guppy` when richer graph queries are needed | Inventory packages and targets, expand changed-crate reverse dependencies, route risk packs, and plan release/CI lanes. |
| Test execution | `cargo-nextest`; `cargo test --doc` | Run normal Rust tests through nextest while keeping doctests explicit because nextest does not run them. |
| Coverage | `cargo-llvm-cov` | Produce execution-surface evidence and artifacts without treating coverage as a correctness claim. |
| Static mutation exposure | `ripr` | Shift weak-oracle detection left on PRs and produce repair packets before runtime mutation is warranted. |
| Runtime mutation | `cargo-mutants` | Provide targeted PR, scheduled, and release mutation backstops without taxing every default PR. |
| Unsafe / UB evidence | `unsafe-review`; Miri | Review unsafe contracts statically and use Miri for targeted concrete UB witnesses. |
| Dependency trust | `cargo-deny`; `cargo-vet`; RustSec / `cargo-audit`; `cargo-auditable` for shipped binaries | Gate advisories, licenses, sources, duplicate/banned crates, and durable dependency audit evidence. |
| Public API / release compatibility | `cargo-semver-checks`; rustdoc JSON for custom API reports | Check release compatibility and build custom public-surface inventories only when product facts need them. |
| Workflow policy | `actionlint`; `zizmor` | Separate workflow syntax/semantic checks from workflow security posture. |
| Text and config hygiene | `taplo`; `typos`; markdown link/style tooling | Keep manifests, policy TOML, spelling, and docs links mechanically checkable. |
| Workspace hygiene | `cargo-udeps` scheduled/manual; `cargo-hakari` only when duplicate-build pain is measured | Keep dependency cleanup and build-graph optimization off the default PR path unless the repo proves the cost is justified. |
| CI cache | `Swatinem/rust-cache` by default; `sccache` only when economics justify it | Prefer simple Cargo-aware caching before introducing remote compiler-cache infrastructure. |

## Authority rules

- `ast-grep` finds candidates; Rust-aware tooling decides authoritative Rust
  identity and policy.
- `git ls-files -z` is the source inventory authority for policy scans unless a
  tool intentionally scans beyond tracked state.
- `cargo_metadata` is the basic Cargo metadata substrate; use `guppy` for richer
  dependency graph, feature graph, changed-crate, and lane-routing queries.
- `ripr` is static mutation-exposure analysis. It does not run mutants or claim
  killed/survived outcomes.
- `cargo-mutants` and Miri are runtime backstops for targeted, scheduled, and
  release lanes, not unconditional default PR taxes.
- Coverage is execution-surface evidence, not test adequacy or release
  readiness by itself.
- Dependency and source exceptions must be receipted with durable ledgers rather
  than hidden in tool flags.

## Standard wrapper surface

Shipper should prefer stable `cargo xtask` names even when the implementation
changes. The intended command families are:

```bash
cargo xtask check-pr
cargo xtask fix-pr
cargo xtask pr-summary

cargo xtask allow-check
cargo xtask allow-diff
cargo xtask ripr-pr
cargo xtask unsafe-review-pr

cargo xtask test-pr
cargo xtask coverage
cargo xtask mutation-targeted
cargo xtask miri-targeted

cargo xtask check-deps
cargo xtask check-supply-chain
cargo xtask semver-check
cargo xtask check-workflows
cargo xtask check-toml
cargo xtask policy-report
```

Not every wrapper exists today. Until a wrapper lands, keep the upstream command
in docs and CI narrowly scoped, then migrate repeatable policy into `xtask` once
the repo has enough evidence to freeze the contract.

## Default lane policy

| Lane | Default stance |
|---|---|
| PR | Required routed Rust-small gate, `ripr` advisory/repair signal, targeted mutation only when labels or risk warrant it. |
| Main | Broader full-CI evidence after merge. |
| Scheduled | Deeper fuzz, proptest, mutation, supply-chain, and hygiene checks. |
| Release | Readiness, semver, security, mutation/Miri evidence where relevant, and auditable artifacts must be clean enough to ship. |

The bottom line is to standardize upstream engines and repo-facing wrappers, but
not every heavyweight engine as default CI.
