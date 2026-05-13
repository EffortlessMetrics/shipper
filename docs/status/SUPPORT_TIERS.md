# Support Tiers

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Linked proposal: SHIPPER-PROP-0001
Linked specs: SHIPPER-SPEC-0001, SHIPPER-SPEC-0002
Linked ADRs: SHIPPER-ADR-0001
Linked plan: plans/0.4.0/spec-system.md
Linked issues: #109, #195
Linked PRs:
Support-tier impact: source of truth
Policy impact: policy-report and future doc-contract report

Support tiers are the claim-proof map for Shipper. README and product docs must
not make stronger claims than this file supports.

## Tier Model

| Tier | Meaning |
|---|---|
| stable | Implemented, tested, documented, and backed by a proof command or artifact. |
| advisory | Useful signal exists, but it is non-blocking or incomplete. |
| experimental | Behavior exists but is not yet a user promise. |
| planned | Roadmap intent only. |
| stable/internal | Stable internal or CI contract, not necessarily a public user promise. |

## Claim Map

| Claim | Tier | Proof / Source | Owner |
|---|---|---|---|
| Manifest-level topological publish planning | stable | Planner regression tests; `shipper plan`; roadmap #109 | engine |
| File-policy enforcement | stable/internal | `cargo xtask check-file-policy --mode blocking-allowlist`; `cargo xtask policy-report`; CI `Policy` job | release/ci |
| Clippy/rustc lint floor | stable/internal | `cargo xtask check-lint-policy`; workspace lints; `cargo clippy --workspace --all-targets --all-features -- -D warnings` | rust/lints |
| No-panic production baseline | stable/internal | `cargo xtask no-panic check`; `policy/no-panic-baseline.toml` | rust/lints |
| ripr advisory signal | advisory | `cargo xtask ripr-pr`; repo-scoped badge artifacts | release/ci |
| Mutation PR lane | advisory | `cargo xtask mutants-pr --changed` | tests |
| 0.4.0 release readiness proof | stable | `docs/release/0.4.0-readiness.md`; `plans/0.4.0/release-readiness-proof.md` | release/ci |
| Ambiguous publish reconciliation | planned | Future registry reconciliation proposal/spec; #102 and #99 | engine |
| Resume under real interruption | planned | Future interruption rehearsal proof | engine |
| Trusted Publishing default | planned/advisory | Release workflow and #96 follow-up | release/ci |

## Rules

- Stable claims need a proof command or artifact.
- Advisory claims may guide maintainers, but must not be described as hard
  release gates unless policy promotes them.
- Planned claims should point to roadmap, proposal, spec, or issue context.
- Internal claims should stay internal unless user-facing proof exists.
- When README or product docs change, update this file or narrow the claim.
