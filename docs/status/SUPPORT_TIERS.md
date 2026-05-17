# Support Tiers

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md
Linked ADRs:
Linked plan:
Linked issues: #109, #195
Linked PRs:
Support-tier impact: source of truth
Policy impact: policy ledgers remain the source of truth for exceptions and receipts
Proof commands: cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check

Support tiers are Shipper's claim-to-proof map. README and product docs must not
make stronger claims than this file supports.

## Tier Model

| Tier | Meaning |
|---|---|
| stable | Implemented, tested, documented, and backed by a proof command or artifact. |
| stable/internal | Stable internal or CI contract, not necessarily a public user promise. |
| advisory | Useful signal exists, but it is non-blocking or incomplete. |
| experimental | Behavior exists, but is not yet a user promise. |
| planned | Roadmap intent only. |

## Claim Map

| Claim | Tier | Proof / Source | Owner |
|---|---|---|---|
| Facade / CLI / core crate boundary | stable/internal | `cargo xtask package-surface` fails if `shipper` stops depending on `shipper-cli`/`shipper-core`, `shipper-cli` stops depending on `shipper-core`, `shipper-core` has any normal/dev/build dependency on `shipper`/`shipper-cli`/`clap`/`indicatif`, or `xtask` is not the only private workspace package; see `docs/architecture.md` and crate manifests | architecture |
| `shipper` install facade | stable | `cargo install --path crates/shipper --locked`; CI `Install Smoke` job; `shipper --version`; `shipper --help`; `shipper doctor --help`; `shipper plan --help`; `shipper preflight --help` | packaging/ux |
| Unversioned `cargo install shipper` from crates.io | planned | Cargo requires `--version` while the public crate is prerelease-only; promote when a non-prerelease `shipper` version is published and smoke-tested | packaging/ux |
| Manifest-level topological publish planning | stable | Planner regression tests; `shipper plan`; roadmap #109 | engine |
| File-policy enforcement | stable/internal | `cargo xtask check-file-policy --mode blocking-allowlist`; `cargo xtask policy-report`; CI `Policy` job | release/ci |
| Rust 1.95 / 0.4 policy floor | stable/internal | Workspace lints; `cargo xtask check-lint-policy`; `cargo clippy --workspace --all-targets --all-features -- -D warnings` | rust/lints |
| No-panic production baseline | stable/internal | `cargo xtask no-panic check`; `policy/no-panic-baseline.json` | rust/lints |
| ripr exposure signal | advisory | `cargo xtask ripr-pr`; repo-scoped badge artifacts | release/ci |
| Mutation PR lane | advisory / opt-in | `cargo xtask mutants-pr --changed` | tests |
| 0.4.0 release readiness proof | stable | `docs/release/0.4.0-readiness.md`; `cargo xtask policy-report`; `cargo publish --dry-run --workspace` | release/ci |
| Ambiguous publish reconciliation | stable | `cargo test -p shipper-core reconcile --lib`; `cargo test -p shipper-core state --lib`; `cargo test -p shipper-cli --test bdd_publish`; `PublishReconciling` / `PublishReconciled` events | engine |
| crates.io first-publish backoff profile | stable | `cargo test -p shipper-core runtime::execution --lib`; `cargo test -p shipper-core publish --lib`; `RegistryProfile::crates_io()` | engine |
| Retry-After retry floor | stable | `cargo test -p shipper-core retry_after --lib`; `cargo test -p shipper-core publish --lib`; raw cargo stderr/stdout retry signal path | engine |
| Preflight registry pacing estimate | stable | `cargo test -p shipper-core estimate_preflight_duration --lib`; `cargo test -p shipper-cli preflight`; `estimated_publish_duration` JSON field | engine/cli |
| Resume after synthetic publish interruption | stable/internal | `cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`; CI `BDD Tests` job; proves persisted `state.json`/`events.jsonl` let `shipper resume` complete without duplicate publishes against fake Cargo and a mock registry | engine |
| Resume under live runner interruption | planned | Manual release-candidate procedure in `docs/how-to/run-recover-rehearsal.md`; promote only after a completed crates.io rehearsal artifact proves cancelled-run artifact recovery and resume on a real runner | engine |
| Trusted Publishing default | planned/advisory | Future Trusted Publishing spec and #96 | release/ci |

## Rules

- Stable claims need a proof command or artifact.
- Advisory claims may guide maintainers, but must not be described as hard
  release gates unless policy promotes them.
- Planned claims should point to roadmap, proposal, spec, or issue context.
- Internal claims should stay internal unless user-facing proof exists.
- When README or product docs change, update this file or narrow the claim.
