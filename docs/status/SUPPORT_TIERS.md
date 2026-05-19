# Support Tiers

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.4.0
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md; docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md; docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md; docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md; docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md; docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
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
| Unversioned `cargo install shipper` from crates.io | planned/advisory | The 0.4.0 docs now show the intended stable install path; promote only after a non-prerelease `shipper` version is published to crates.io and `cargo install shipper --locked` is smoke-tested against that public artifact | packaging/ux |
| Manifest-level topological publish planning | stable | Planner regression tests; `shipper plan`; roadmap #109 | engine |
| Plan JSON publish graph | stable | `cargo test -p shipper-cli --test bdd_workflow given_multi_crate_when_plan_json_then_valid_json_output`; `shipper plan --format json` emits `shipper.plan.v1` | cli/integrations |
| JSON evidence compatibility contract | stable/internal | `docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md`; `plans/0.4.0/json-evidence-contracts.md`; `cargo xtask check-doc-contracts --mode advisory` | cli/integrations |
| File-policy enforcement | stable/internal | `cargo xtask check-file-policy --mode blocking-allowlist`; `cargo xtask policy-report`; CI `Policy` job | release/ci |
| Rust 1.95 / 0.4 policy floor | stable/internal | Workspace lints; `cargo xtask check-lint-policy`; `cargo clippy --workspace --all-targets --all-features -- -D warnings` | rust/lints |
| No-panic production baseline | stable/internal | `cargo xtask no-panic check`; `policy/no-panic-baseline.json` | rust/lints |
| ripr exposure signal | advisory | `cargo xtask ripr-pr`; repo-scoped badge artifacts | release/ci |
| Mutation PR lane | advisory / opt-in | `cargo xtask mutants-pr --changed` | tests |
| 0.4.0 release readiness proof | planned/advisory | `docs/release/0.4.0-readiness.md` currently records historical `0.4.0-rc.1` evidence and a pending stable proof placeholder; promote only after fresh `0.4.0` version, commit, plan, preflight, policy, install smoke, and publish dry-run evidence are recorded before tag | release/ci |
| Ambiguous publish reconciliation | stable | `cargo test -p shipper-core reconcile --lib`; `cargo test -p shipper-core state --lib`; `cargo test -p shipper-cli --test bdd_publish`; `PublishReconciling` / `PublishReconciled` events | engine |
| crates.io first-publish backoff profile | stable | `cargo test -p shipper-core runtime::execution --lib`; `cargo test -p shipper-core publish --lib`; `RegistryProfile::crates_io()` | engine |
| Retry-After retry floor | stable | `cargo test -p shipper-core retry_after --lib`; `cargo test -p shipper-core publish --lib`; raw cargo stderr/stdout retry signal path | engine |
| Preflight registry pacing estimate | stable | `cargo test -p shipper-core estimate_preflight_duration --lib`; `cargo test -p shipper-cli preflight`; `estimated_publish_duration` JSON field | engine/cli |
| Status JSON registry comparison | stable | `cargo test -p shipper-cli --test e2e_status status_json_format_produces_registry_report`; `shipper status --format json` emits `shipper.status.v1` registry/package state | cli/integrations |
| Status watch JSON progress | stable | `cargo test -p shipper-cli status_watch_report_summarizes_state_and_scheduled_events --lib`; `shipper status --watch --format json` emits `shipper.status.watch.v1` progress state | cli/integrations |
| Release black-box recorder hardening | stable/internal | `docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md`; `plans/0.4.0/release-operator-visibility-and-survive-proof.md`; `cargo test -p shipper-cli inspect_events --lib --locked`; `cargo test -p shipper-core drift --lib --locked`; `cargo test -p shipper-core rebuild --lib --locked`; GitHub Actions `Live runner interruption rehearsal` run 26051581056 with `shipper-live-interruption-seed-26051581056` and `shipper-live-interruption-resume-26051581056` artifacts | engine/cli |
| Doctor JSON diagnostics | stable | `cargo test -p shipper-cli --test e2e_doctor doctor_json_format_reports_diagnostics_without_token_value`; `shipper doctor --format json` emits `shipper.doctor.v1` without token values | cli/integrations |
| Idempotent workspace publish | stable | `docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md`; `plans/0.4.0/idempotent-workspace-publish.md`; `docs/how-to/publish-missing-workspace-crates.md`; `cargo test -p shipper-cli --test bdd_publish --locked`; `cargo test -p shipper-cli --test e2e_publish --locked` proves all-existing skip, mixed existing/missing success, failure preservation, and publish JSON package-state counts; this is registry `name@version` idempotency, not source-diff or changed-crate detection | cli/integrations |
| Publish JSON command envelope | stable | `cargo test -p shipper-cli --test e2e_publish publish_json_format_writes_command_envelope_to_stdout`; `shipper publish --format json` emits `shipper.publish.v1` with package summary, artifact paths, and nested receipt evidence for the targeted registry | cli/integrations |
| Resume JSON command envelope | stable | `cargo test -p shipper-cli --test bdd_resume given_pending_state_when_resume_json_then_stdout_is_command_envelope`; `shipper resume --format json` emits `shipper.resume.v1` with safety summary, package counts, artifact paths, and nested receipt evidence for the targeted registry | cli/integrations |
| Resume after synthetic publish interruption | stable/internal | `cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`; CI `BDD Tests` job; proves persisted `state.json`/`events.jsonl` let `shipper resume` complete without duplicate publishes against fake Cargo and a mock registry | engine |
| Resume under live runner interruption | stable/internal | GitHub Actions `Live runner interruption rehearsal` run 26051581056 uploaded `shipper-live-interruption-seed-26051581056` and `shipper-live-interruption-resume-26051581056`; `cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`; proves real runner artifact handoff and safe resume against fake Cargo/mock registry proof surfaces, not crates.io publication | engine |
| Trusted Publishing prerequisite diagnostics | advisory | `docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md`; `shipper doctor`; `cargo test -p shipper-cli --test cli_e2e doctor_command_detects_trusted_publishing_auth`; inspects visible GitHub OIDC env and release workflow prerequisites without claiming crates.io-side registration proof | release/ci |
| Release auth evidence in receipts/events | stable/internal | `cargo test -p shipper-core collect_auth_evidence --lib`; `cargo test -p shipper-core run_publish_receipt_contains_evidence_after_success --lib`; `cargo test -p shipper-core event_types_serialize_correctly --lib`; `AuthEvidenceRecorded` events and `receipt.auth_evidence` record observed auth context without token values or token-provenance overclaiming | engine/release |
| Release auth workflow proof artifact | stable/internal | GitHub Actions `Release` workflow run `26072938626` uploaded `shipper-rehearse-26072938626` with `.shipper/auth-evidence.json`; the artifact records `shipper.release_auth_evidence.v1`, `auth_action.outcome = "failure"`, `fallback.configured = true`, `fallback.used = true`, and `selected_token_source = "fallback_secret"` without token values; proves current fallback evidence only, not Trusted Publishing default | release/ci |
| Long-lived token fallback warnings | advisory | `docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md`; `cargo test -p shipper-core run_preflight_warns_when_token_auth_overrides_oidc --lib`; `cargo test -p shipper-cli --test cli_e2e doctor_command_warns_when_token_fallback_is_configured`; warns when Cargo token auth wins while Trusted Publishing signals or fallback config are present | release/ci |
| Trusted Publishing default | planned/advisory | `docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md`; `plans/0.4.0/release-auth-evidence-and-trusted-publishing.md`; promote only after release evidence proves the short-lived-token path is the normal path and token fallback state is explicit | release/ci |
| Cargo yank containment primitive | stable/internal | `cargo test -p shipper-core cargo_yank --lib --locked`; proves command construction, registry selection, output capture, and nonzero exit handling for the internal `cargo yank` wrapper; containment is not undo and no live crates.io yank runs in PR CI | engine |
| Reverse-topological yank planning engine | stable/internal | `cargo test -p shipper-core plan_yank --lib --locked`; proves all-published, compromised-only, starting-crate graph mode, explicit reasons, and yank-plan JSON roundtrip for the planning engine | engine |
| Fix-forward planning engine | stable/internal | `cargo test -p shipper-core fix_forward --lib --locked`; proves compromised published receipt filtering, topological successor ordering, empty-compromise handling, and human text rendering; this plans successors but does not edit manifests or publish | engine |
| Remediation receipt marker shape | stable/internal | `cargo test -p shipper-types package_receipt_roundtrip --lib --locked`; `cargo test -p shipper-types receipt_roundtrip --lib --locked`; proves `compromised_at`, `compromised_by`, and `superseded_by` stay in the durable receipt serialization surface | types |
| Remediation CLI command contracts | advisory | `cargo test -p shipper-cli --test e2e_expanded --locked help_yank_snapshot`; `cargo test -p shipper-cli --test e2e_expanded --locked help_plan_yank_snapshot`; `cargo test -p shipper-cli --test e2e_expanded --locked help_fix_forward_snapshot`; command help is snapshotted, but `shipper yank` execution and full remediation orchestration are not yet promoted | cli |
| Remediation JSON command envelopes | stable/internal | `cargo test -p shipper-cli --test e2e_expanded --locked plan_yank_json_format_emits_schema_version`; `cargo test -p shipper-cli --test e2e_expanded --locked fix_forward_json_format_emits_schema_version`; `plan-yank --format json` emits `shipper.plan_yank.v1` and `fix-forward --format json` emits `shipper.fix_forward.v1`, each with top-level planning fields plus `schema_version` and `command`; this does not emit `.shipper/remediation-plan.json` or execute yanks/fix-forward publishes | cli/integrations |
| Remediation dry-run artifact | stable/internal | `cargo test -p shipper-core remediation --lib --locked`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_dry_run_writes_remediation_plan_artifact`; `shipper remediate --dry-run` writes `.shipper/remediation-plan.json` as `shipper.remediation_plan.v1` with source receipt, target crate/version, affected packages, yank order, fix-forward suggestions, risk notes, and command sequence; operator-supplied reason text is omitted from durable artifacts; this does not execute yanks, edit manifests, or publish successors | cli/integrations |
| Guarded remediation plan execution | stable/internal | `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_executes_reviewed_plan_with_fake_cargo`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_halts_on_failed_yank`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_redacts_event_reason`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_requires_state_dir_plan`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_rejects_registry_mismatch`; PR #352 CI; proves `shipper remediate --execute-plan .shipper/remediation-plan.json` executes reviewed containment yanks against fake Cargo, rejects plans outside the configured state dir, rejects registry mismatches, validates yank identifiers, emits `PackageYanked` event evidence, halts on the first failed yank, and records only the redacted reason placeholder; this does not prove live crates.io yank execution or fix-forward publishing | engine/cli |
| Receipt-driven remediation | advisory | `docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md`; `plans/0.4.0/receipt-driven-remediation.md`; bounded primitives, dry-run artifacts, and fake-Cargo guarded execution are mapped to proof, but full mechanical remediation remains planned until live-operator yank evidence and fix-forward execution semantics are deliberately promoted | engine/cli |

## Rules

- Stable claims need a proof command or artifact.
- Advisory claims may guide maintainers, but must not be described as hard
  release gates unless policy promotes them.
- Planned claims should point to roadmap, proposal, spec, or issue context.
- Internal claims should stay internal unless user-facing proof exists.
- When README or product docs change, update this file or narrow the claim.
