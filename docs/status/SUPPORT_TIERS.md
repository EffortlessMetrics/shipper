# Support Tiers

Status: accepted
Owner: EffortlessMetrics
Created: 2026-05-13
Milestone: 0.5.0 release-line stabilization
Linked proposal: docs/proposals/SHIPPER-PROP-0001-source-of-truth-and-release-evidence.md
Linked specs: docs/specs/SHIPPER-SPEC-0001-source-of-truth-stack.md; docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md; docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md; docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md; docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md; docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md
Linked ADRs:
Linked plan:
Linked issues: #109, #153, #195, #202, #205, #419, #420, #421, #422
Linked PRs:
Support-tier impact: source of truth
Policy impact: policy ledgers remain the source of truth for exceptions and receipts
Proof commands: cargo xtask check-doc-contracts --mode advisory; cargo xtask check-file-policy --mode blocking-allowlist; cargo xtask policy-report; cargo fmt --all -- --check

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

## Release Lifecycle

Support maturity and release lifecycle are separate axes. A `stable` command
claim means the named behavior has implementation and proof; it does not prove
that an unreleased source snapshot is available from crates.io. Use these
lifecycle qualifiers when a claim depends on an artifact:

| Lifecycle qualifier | Meaning |
|---|---|
| historical public | Proved by an already-published tag/crate/asset, currently the 0.4.0 line |
| candidate | Present on an exact reviewed swarm source identity; not public release proof |
| release-authority preparation | Promotion/rehearsal evidence in `EffortlessMetrics/shipper`; still not publication authorization |
| public | Proved after tag, crates.io/GitHub Release, and public-install verification |

Repository source, a README, a green swarm run, or a `stable/internal` row must
never be used alone to advance a claim to a later lifecycle state.

## Development Evidence

### October 26 audit and November 2 development disposition

The audit at swarm source `8e8cc401d02cc239d1db808ff800850118d11f39`
demonstrated the defects below. Its negative controls remain historical evidence
for that source; they must not be presented as still-open implementation work
after the October repair train. At November 2, development source
`bdf266842e78580b24f8b8ff998eb31e9cac6255` passed its required routed
Rust check, but is **NOT FROZEN** and has **no publication authorization**.
The separate release-authority `shipper/main` has not received those fixes,
and the latest proved public install remains the 0.4.0 line.

| Claim / original audit issue | Landed development correction | Proof still required |
|---|---|---|
| Effective registry determines graph and plan ID ([#409](https://github.com/EffortlessMetrics/shipper-swarm/issues/409)) | [PR #422](https://github.com/EffortlessMetrics/shipper-swarm/pull/422) merged; original mismatch regression retained | Full candidate and selected consumer recheck |
| Receipt attributes the selected workspace ([#413](https://github.com/EffortlessMetrics/shipper-swarm/issues/413)) | [PR #420](https://github.com/EffortlessMetrics/shipper-swarm/pull/420) merged, including conflicting inherited Git settings | Exact release-authority consumer identity |
| Unknown registry observations cannot become upload absence ([#402](https://github.com/EffortlessMetrics/shipper-swarm/issues/402)) | [PRs #429](https://github.com/EffortlessMetrics/shipper-swarm/pull/429) and [#430](https://github.com/EffortlessMetrics/shipper-swarm/pull/430) merged the unknown/Retryable reconciliation paths | Candidate compatibility/security/recovery matrix; no blind retry |
| Retained-state admission and full-plan completion ([#403](https://github.com/EffortlessMetrics/shipper-swarm/issues/403), [#415](https://github.com/EffortlessMetrics/shipper-swarm/issues/415)) | [PRs #436](https://github.com/EffortlessMetrics/shipper-swarm/pull/436) and [#423](https://github.com/EffortlessMetrics/shipper-swarm/pull/423) merged | Actual independent cross-run second continuation under [#418](https://github.com/EffortlessMetrics/shipper-swarm/issues/418), then source-authority consumption |
| Ordered visible retry notices ([#414](https://github.com/EffortlessMetrics/shipper-swarm/issues/414)) | [PR #450](https://github.com/EffortlessMetrics/shipper-swarm/pull/450) merged; actual core/CLI stream and retained-outcome controls passed | Full candidate and public-release claims remain separate |
| Process output, timeout and cleanup ([#412](https://github.com/EffortlessMetrics/shipper-swarm/issues/412)) | Draining, typed timeout, bounded capture, and Linux/macOS process-group cleanup landed through #427/#435/#438/#444; [#484](https://github.com/EffortlessMetrics/shipper-swarm/pull/484) subsequently added Windows Job Object cleanup, confirmation of both pipes' EOF, and executable-only console interruption handling. See [failure modes](../failure-modes.md#windows-subprocess-cleanup). | Focused native Linux and Windows tests cover owned cleanup and interruption, including OS-delivered Windows Ctrl+Break with fixture Cargo and a loopback registry. Native macOS runtime and final release-binary qualification remain outstanding; this is not a public support-tier promotion. |
| Archived executable verification ([#417](https://github.com/EffortlessMetrics/shipper-swarm/issues/417)) | [PR #431](https://github.com/EffortlessMetrics/shipper-swarm/pull/431) merged synthetic/verifier controls | Actual optimized archive execution on four matching native platforms; independently repair [#454](https://github.com/EffortlessMetrics/shipper-swarm/issues/454)'s observed inherited-Git-context source binding |
| Full current candidate ([#370](https://github.com/EffortlessMetrics/shipper-swarm/issues/370), [#255](https://github.com/EffortlessMetrics/shipper-swarm/issues/255)) | The required Rust small check passed; existing full hosted alternative is implemented | Exact-source broad/semver/package/compatibility/auth/release matrix, then freeze |

The statuses above describe **development implementation**, not a support-tier
promotion. Historical `stable` and focused rows retain only their cited
subjects. The complete #255 candidate, release-authority #475–#479
promotion/rehearsal/explicit GO/public proof, and observed consumer adoption
are separate lifecycle outcomes. Do not equate a merged fix, synthetic archive,
same-run transport, scheduled/cancelled run, or checked-in README with public
0.5.0 availability.

## Claim Map

| Claim | Tier | Proof / Source | Owner |
|---|---|---|---|
| Facade / CLI / core crate boundary | stable/internal | `cargo xtask package-surface` fails if `shipper` stops depending on `shipper-cli`/`shipper-core`, `shipper-cli` stops depending on `shipper-core`, `shipper-core` has any normal/dev/build dependency on `shipper`/`shipper-cli`/`clap`/`indicatif`, or `xtask` is not the only private workspace package; see `docs/architecture.md` and crate manifests | architecture |
| `shipper` install facade | stable | `cargo install --path crates/shipper --locked`; CI `Install Smoke` job; `shipper --version`; `shipper --help`; `shipper doctor --help`; `shipper plan --help`; `shipper preflight --help` | packaging/ux |
| Unversioned `cargo install shipper` from crates.io | stable | `cargo install shipper --locked --root target/install-smoke-0.4.0-unversioned --force`; installed binary reported `shipper 0.4.0`; `cargo install shipper --version 0.4.0 --locked --root target/install-smoke-0.4.0-versioned --force` also passed against the public crates.io artifact | packaging/ux |
| Manifest-level topological publish planning | stable | Planner regression tests; `shipper plan`; roadmap #109 | engine |
| Plan JSON publish graph | stable | `cargo test -p shipper-cli --test bdd_workflow given_multi_crate_when_plan_json_then_valid_json_output`; `shipper plan --format json` emits `shipper.plan.v1` | cli/integrations |
| JSON evidence compatibility contract | stable/internal | `docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md`; `plans/0.4.0/json-evidence-contracts.md`; `cargo xtask check-doc-contracts --mode advisory` | cli/integrations |
| File-policy enforcement | stable/internal | `cargo xtask check-file-policy --mode blocking-allowlist`; `cargo xtask policy-report`; CI `Policy` job | release/ci |
| Rust 1.95 / 0.4 policy floor | stable/internal | Workspace lints; `cargo xtask check-lint-policy`; `cargo clippy --workspace --all-targets --all-features -- -D warnings` | rust/lints |
| No-panic production baseline | stable/internal | `cargo xtask no-panic check`; `policy/no-panic-baseline.json` | rust/lints |
| ripr exposure signal | advisory | `cargo xtask ripr-pr`; repo-scoped badge artifacts | release/ci |
| Mutation PR lane | advisory | `cargo xtask mutants-pr --changed`; maintainer-controlled opt-in on PRs labeled `mutation` or `full-ci`; `.github/workflows/mutation.yml`; `policy/ci-trigger-labels.toml` plus `cargo xtask ci-labels check`/`check-live` own taxonomy and drift proof | tests |
| `shipper-swarm` routed PR gate | stable/internal | `docs/ci/test-evidence-lanes.md`; `docs/status/SWARM_OPERATION.md`; `.github/workflows/em-ci-routed-rust.yml`; `gh api repos/EffortlessMetrics/shipper-swarm/branches/main/protection/required_status_checks` shows only `Shipper Rust Small Result` is required; `gh api repos/EffortlessMetrics/shipper-swarm --jq '{allow_squash_merge,allow_merge_commit,allow_rebase_merge,allow_auto_merge,delete_branch_on_merge}'` shows squash-only normal PR merges with auto-merge and branch deletion; routed Rust is the meaningful required gate, while substantive review is performed through the active Codex, Claude, or human route because Factory Droid is retired; current swarm policy blocks silent GitHub-hosted fallback and must not be inferred for `EffortlessMetrics/shipper` release-authority syncs | release/ci |
| 0.4.0 release readiness proof | stable | `docs/release/0.4.0-readiness.md` records the `0.4.0` version/commit/plan/preflight/policy/install/dry-run/rehearsal proof, tag `v0.4.0`, Release workflow run `26141754574`, final `.shipper` evidence, and public crates.io install smoke; Trusted Publishing default and the missing `x86_64-apple-darwin` binary asset remain explicit carry-over | release/ci |
| Ambiguous publish reconciliation | stable | Historical/focused contract proof: `cargo test -p shipper-core reconcile --lib`; `cargo test -p shipper-core state --lib`; `cargo test -p shipper-cli --test bdd_publish`; `PublishReconciling` / `PublishReconciled` events. The [#402](https://github.com/EffortlessMetrics/shipper-swarm/issues/402) correction landed through [PR #429](https://github.com/EffortlessMetrics/shipper-swarm/pull/429) and [PR #430](https://github.com/EffortlessMetrics/shipper-swarm/pull/430); retain focused refusal controls in [#255](https://github.com/EffortlessMetrics/shipper-swarm/issues/255)'s exact-candidate proof. This row does not qualify public 0.5.0. | engine |
| Rehearsal configured-authority isolation | stable/internal | `cargo test -p shipper-registry rehearsal_separation --locked`; `cargo test -p shipper-config rehearsal_authority_validation --locked`; `cargo test -p shipper-core run_rehearsal_rejects --locked`; `cargo test -p shipper-cli --test e2e_rehearse rehearsal_authority --locked`; rejects same names, live API/index family overlap, and crates.io-family configured hosts before rehearsal state/network/publish-smoke side effects; does not prove DNS, resolved-address, namespace, or administrative isolation and is not publication evidence | engine/cli |
| crates.io first-publish backoff profile | stable | `cargo test -p shipper-core runtime::execution --lib`; `cargo test -p shipper-core publish --lib`; `RegistryProfile::crates_io()` | engine |
| Retry-After retry floor | stable | `cargo test -p shipper-core retry_after --lib`; `cargo test -p shipper-core publish --lib`; raw cargo stderr/stdout retry signal path | engine |
| Preflight registry pacing estimate | stable | `cargo test -p shipper-core estimate_preflight_duration --lib`; `cargo test -p shipper-cli preflight`; `estimated_publish_duration` JSON field | engine/cli |
| Status JSON registry comparison | stable | `cargo test -p shipper-cli --test e2e_status status_json_format_produces_registry_report`; `cargo test -p shipper-cli --test e2e_status status_completed_outcome_has_human_json_parity_without_side_effect_claims`; `cargo test -p shipper-cli --test e2e_status status_plans_packages_for_each_effective_registry`; `cargo test -p shipper-cli --test e2e_status status_multi_registry_reports_registry_specific_plan_identity`; `shipper status --format json` emits registry-aware `shipper.status.v1` package state, per-registry plan identity, and a read-only typed outcome | cli/integrations |
| Status watch JSON progress | stable | `cargo test -p shipper-cli status_watch_report_summarizes_state_and_scheduled_events --lib`; `shipper status --watch --format json` emits `shipper.status.watch.v1` progress state | cli/integrations |
| Durable local status outcome | stable/internal | `cargo test -p shipper-cli status_durable::tests --lib`; `cargo test -p shipper-cli --test e2e_status durable_status_no_evidence_has_human_json_parity_without_registry_access`; `cargo test -p shipper-cli --test e2e_status_durable --locked`; `shipper status --durable --format json` emits `shipper.status.durable.v1` from local events/state/receipt/reconciliation (including configured encrypted state/receipt) and affirmative core liveness evidence without registry access or publication; the doc-hidden `shipper_core::cli_bridge` is an additive unsupported CLI-integration seam and is not re-exported by `shipper` | engine/cli |
| Release black-box recorder hardening | stable/internal | `docs/specs/SHIPPER-SPEC-0005-release-operator-visibility-and-survive-proof.md`; `plans/0.4.0/release-operator-visibility-and-survive-proof.md`; `cargo test -p shipper-cli inspect_events --lib --locked`; `cargo test -p shipper-core drift --lib --locked`; `cargo test -p shipper-core rebuild --lib --locked`; GitHub Actions `Live runner interruption rehearsal` run 26051581056 with `shipper-live-interruption-seed-26051581056` and `shipper-live-interruption-resume-26051581056` artifacts; `cargo test -p shipper-core --lib file_sync --locked` proves checked event/temporary-file synchronization failures and ordering in ordinary plaintext publish/resume. [Persistence limits](../INVARIANTS.md#persistence-boundary-and-failure-model) distinguish returned errors, process interruption, artifact transfer, best-effort directory sync, encrypted helpers, and unqualified host/power loss; this adds no platform or release-tier promotion | engine/cli |
| Doctor JSON diagnostics | stable | `cargo test -p shipper-cli --test e2e_doctor doctor_json_format_reports_diagnostics_without_token_value`; `shipper doctor --format json` emits `shipper.doctor.v1` without token values | cli/integrations |
| Command-owned operator outcomes | stable | `docs/specs/SHIPPER-SPEC-0004-json-evidence-contracts.md`; `cargo test -p shipper-cli --test e2e_publish completed_partial_publish_human_and_json_have_semantic_parity`; `cargo test -p shipper-cli --test e2e_status status_completed_outcome_has_human_json_parity_without_side_effect_claims`; `cargo test -p shipper-cli --test e2e_status durable_status_no_evidence_has_human_json_parity_without_registry_access`; doctor, plan, preflight, publish, resume, non-watch status, and durable status each preserve their own typed result/status and human/JSON next-action semantics where present; identity, safety, and evidence fields remain command-specific, and non-watch status does not claim durable evidence or rerun safety | cli/integrations |
| Read-only evidence inspection | stable | `cargo test -p shipper-cli inspect_events --lib --locked`; `cargo test -p shipper-cli --test cli_e2e inspect_receipt_command_displays_new_fields`; `cargo test -p shipper-cli --test e2e_expanded help_inspect_events_snapshot`; `cargo test -p shipper-cli --test e2e_expanded help_inspect_receipt_snapshot`; `shipper inspect-events`, `shipper inspect-receipt`, and the three status modes remain separate contracts; event history alone is not liveness proof, and durable status bypasses registry access | cli/integrations |
| Publish/resume process exit vocabulary | stable | `cargo test -p shipper-cli --test e2e_publish --locked`; `cargo test -p shipper-cli --test bdd_resume --locked`; finalized success exits `0`, finalized partial result exits `2` with receipt evidence, and pre-receipt errors exit `1`; Clap usage also exits `2` but creates no execution evidence | cli/integrations |
| Idempotent workspace publish | stable | `docs/specs/SHIPPER-SPEC-0007-idempotent-workspace-publish.md`; `plans/0.4.0/idempotent-workspace-publish.md`; `docs/how-to/publish-missing-workspace-crates.md`; `cargo test -p shipper-cli --test bdd_publish --locked`; `cargo test -p shipper-cli --test e2e_publish --locked` proves all-existing skip, mixed existing/missing success, failure preservation, and publish JSON package-state counts; this is registry `name@version` idempotency, not source-diff or changed-crate detection | cli/integrations |
| Publish JSON command envelope | stable | `cargo test -p shipper-cli --test e2e_publish publish_json_format_writes_command_envelope_to_stdout`; `shipper publish --format json` emits `shipper.publish.v1` with package summary, safe rerun posture, artifact paths, and nested receipt evidence for the targeted registry | cli/integrations |
| Resume JSON command envelope | stable | `cargo test -p shipper-cli --test bdd_resume given_pending_state_when_resume_json_then_stdout_is_command_envelope`; `shipper resume --format json` emits `shipper.resume.v1` with safety summary, package counts, artifact paths, and nested receipt evidence for the targeted registry | cli/integrations |
| Resume after synthetic publish interruption | stable/internal | `cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`; CI `BDD Tests` job; proves persisted `state.json`/`events.jsonl` let `shipper resume` complete without duplicate publishes against fake Cargo and a mock registry | engine |
| Resume under live runner interruption | stable/internal | GitHub Actions `Live runner interruption rehearsal` run 26051581056 uploaded `shipper-live-interruption-seed-26051581056` and `shipper-live-interruption-resume-26051581056`; `cargo test -p shipper-cli --test e2e_rehearse -- --nocapture`; proves real runner artifact handoff and safe resume against fake Cargo/mock registry proof surfaces, not crates.io publication | engine |
| Trusted Publishing prerequisite diagnostics | advisory | `docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md`; `shipper doctor`; `cargo test -p shipper-cli --test cli_e2e doctor_command_detects_trusted_publishing_auth`; inspects visible GitHub OIDC env and release workflow prerequisites without claiming crates.io-side registration proof | release/ci |
| Release auth evidence in receipts/events | stable/internal | `cargo test -p shipper-core collect_auth_evidence --lib`; `cargo test -p shipper-core run_publish_receipt_contains_evidence_after_success --lib`; `cargo test -p shipper-core event_types_serialize_correctly --lib`; `AuthEvidenceRecorded` events and `receipt.auth_evidence` record observed auth context without token values or token-provenance overclaiming | engine/release |
| Release auth workflow proof artifact | stable/internal | GitHub Actions `Release` workflow run `26141754574` uploaded `shipper-state-final` with `.shipper/auth-evidence.json`; the artifact records `shipper.release_auth_evidence.v1`, `auth_action.outcome = "failure"`, `fallback.configured = true`, `fallback.used = true`, and `selected_token_source = "fallback_secret"` without token values; proves current fallback evidence only, not Trusted Publishing default | release/ci |
| Long-lived token fallback warnings | advisory | `docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md`; `cargo test -p shipper-core run_preflight_warns_when_token_auth_overrides_oidc --lib`; `cargo test -p shipper-cli --test cli_e2e doctor_command_warns_when_token_fallback_is_configured`; warns when Cargo token auth wins while Trusted Publishing signals or fallback config are present | release/ci |
| Trusted Publishing default | planned | `docs/specs/SHIPPER-SPEC-0006-release-auth-evidence-and-trusted-publishing.md`; `plans/0.4.0/release-auth-evidence-and-trusted-publishing.md`; advisory diagnostics and fallback evidence exist, but default promotion requires release evidence proving the short-lived-token path is the normal path and token fallback state is explicit | release/ci |
| Cargo yank containment primitive | stable/internal | `cargo test -p shipper-core cargo_yank --lib --locked`; proves command construction, registry selection, output capture, and nonzero exit handling for the internal `cargo yank` wrapper; containment is not undo and no live crates.io yank runs in PR CI | engine |
| Reverse-topological yank planning engine | stable/internal | `cargo test -p shipper-core plan_yank --lib --locked`; proves all-published, compromised-only, starting-crate graph mode, explicit-over-receipt reason precedence across every mode, and yank-plan JSON roundtrip for the planning engine | engine |
| Fix-forward planning engine | stable/internal | `cargo test -p shipper-core fix_forward --lib --locked`; proves compromised published receipt filtering, topological successor ordering, empty-compromise handling, and human text rendering; this plans successors but does not edit manifests or publish | engine |
| Remediation receipt marker shape | stable/internal | `cargo test -p shipper-types package_receipt_roundtrip --lib --locked`; `cargo test -p shipper-types receipt_roundtrip --lib --locked`; proves `compromised_at`, `compromised_by`, and `superseded_by` stay in the durable receipt serialization surface | types |
| Direct yank CLI execution | stable/internal | `cargo test -p shipper-cli --test e2e_expanded --locked yank_single_command_records_package_yanked_event_with_fake_cargo`; `cargo test -p shipper-cli --test e2e_expanded --locked yank_plan_execution_runs_entries_in_order_with_fake_cargo`; `cargo test -p shipper-cli --test e2e_expanded --locked yank_plan_execution_halts_on_failed_yank`; proves direct `shipper yank` and reviewed `shipper yank --plan` execution against fake Cargo, event evidence, plan ordering, and halt-on-failure behavior; this does not prove live crates.io yank execution | cli |
| Remediation CLI command contracts | advisory | `cargo test -p shipper-cli --test e2e_expanded --locked help_yank_snapshot`; `cargo test -p shipper-cli --test e2e_expanded --locked help_plan_yank_snapshot`; `cargo test -p shipper-cli --test e2e_expanded --locked help_fix_forward_snapshot`; command help is snapshotted, and direct yank execution is covered separately, but full remediation orchestration remains bounded by the dry-run/fake-Cargo proof surfaces below | cli |
| Remediation JSON command envelopes | stable/internal | `cargo test -p shipper-cli --test e2e_expanded --locked plan_yank_json_format_emits_schema_version`; `cargo test -p shipper-cli --test e2e_expanded --locked fix_forward_json_format_emits_schema_version`; `plan-yank --format json` emits `shipper.plan_yank.v1` and `fix-forward --format json` emits `shipper.fix_forward.v1`, each with top-level planning fields plus `schema_version` and `command`; this does not emit `.shipper/remediation-plan.json` or execute yanks/fix-forward publishes | cli/integrations |
| Remediation dry-run artifact | stable/internal | `cargo test -p shipper-core remediation --lib --locked`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_dry_run_writes_remediation_plan_artifact`; `shipper remediate --dry-run` writes `.shipper/remediation-plan.json` as `shipper.remediation_plan.v1` with source receipt, target crate/version, affected packages, yank order, fix-forward suggestions, risk notes, and command sequence; operator-supplied reason text is omitted from durable artifacts; this does not execute yanks, edit manifests, or publish successors | cli/integrations |
| Guarded remediation plan execution | stable/internal | `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_executes_reviewed_plan_with_fake_cargo`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_halts_on_failed_yank`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_redacts_event_reason`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_requires_state_dir_plan`; `cargo test -p shipper-cli --test e2e_expanded --locked remediate_guarded_execution_rejects_registry_mismatch`; PR #352 CI; proves `shipper remediate --execute-plan .shipper/remediation-plan.json` executes reviewed containment yanks against fake Cargo, rejects plans outside the configured state dir, rejects registry mismatches, validates yank identifiers, emits `PackageYanked` event evidence, halts on the first failed yank, and records only the redacted reason placeholder; this does not prove live crates.io yank execution or fix-forward publishing | engine/cli |
| Containment registry and workspace context | stable/internal | `cargo test -p shipper-cli --test e2e_expanded containment_context --locked`; fake-Cargo single yank, yank-plan and guarded remediation tests check exact registry arguments including crates.io under another Cargo default, legacy configuration and CLI override, selected workspace cwd, all state path forms, and mutation-free selector/plan refusals. This does not prove live registry yanks or evidence-write durability | engine/cli |
| Receipt-driven remediation | advisory | `docs/specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md`; `plans/0.4.0/receipt-driven-remediation.md`; bounded primitives, dry-run artifacts, and fake-Cargo guarded execution are mapped to proof, but full mechanical remediation remains planned until live-operator yank evidence and fix-forward execution semantics are deliberately promoted | engine/cli |

## 0.5.0 stabilization claims

| Claim | Tier | Proof / Source | Owner |
|---|---|---|---|
| Shared sequential/parallel package execution authority | stable/internal | `plans/0.5.0-scheduler-conformance.md`; `mode_parity_corpus_sequential_matches_parallel`; merged exact-head conformance PRs #220, #221, and #227–#234 | engine |
| One readiness polling authority | stable/internal | `docs/INVARIANTS.md`; `shipper_registry::RegistryClient::is_version_visible_with_backoff_and_events`; issue #202 closeout | engine/registry |
| Validated registry destination and redirect boundary | stable/internal | Registry policy tests and merged PRs #217/#219; issue #419 release-authority promotion remains pending | security/release |
| Context-aware webhook and authorization redaction | stable/internal | PR #209 and webhook/non-disclosure test matrix; swarm issue #205 and release issue #421 promotion remain pending | security/observability |
| Webhook transport diagnostic non-disclosure | stable/internal | `cargo test -p shipper-webhook --test transport_privacy --locked`; `cargo test -p shipper-core --lib core_warnings_do_not_disclose_webhook_transport_secrets --locked`; local failure fixtures cover public error formatting and all three core warning paths, with HTTP status or safe failure categories retained; response bodies and underlying HTTP error chains are omitted. This does not prove live delivery or change optional notification semantics | security/observability |
| KDF v1 compatibility and KDF v2 stronger-default writes | stable/internal | `shipper-encrypt` compatibility tests; issue #420 promotion remains pending | security/state |
| Blank OIDC values are unavailable | stable/internal | OIDC detection tests and issue #422 implementation; release-authority promotion remains pending | release/auth |
| 0.4 artifact readability and 0.5 rebuild contract | stable/internal | `docs/INVARIANTS.md`, `docs/reference/state-files.md`, state/rebuild tests, and release evidence gate | state/recovery |

## Rules

- Stable claims need a proof command or artifact.
- Advisory claims may guide maintainers, but must not be described as hard
  release gates unless policy promotes them.
- Planned claims should point to roadmap, proposal, spec, or issue context.
- Internal claims should stay internal unless user-facing proof exists.
- When README or product docs change, update this file or narrow the claim.
