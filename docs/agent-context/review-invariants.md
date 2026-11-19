# Review Invariants

This file captures durable invariants for human, Claude Code, and Codex review of shipper PRs, plus a restoration record for the retired Factory Droid lane. It is shared semantic context, not an executable review authority; each active provider's native skill carries its complete operating procedure.

## Product invariants

- **`unsafe_code = "forbid"` is enforced workspace-wide.** No `unsafe` blocks anywhere.
- **Edition 2024, MSRV 1.95, resolver v3.** Bumping the MSRV is a semver-significant operation. Changing it requires a coordinated update to `Cargo.toml`, `rust-toolchain.toml`, `clippy.toml`, CI workflows (`ci.yml`, `coverage.yml`, `release.yml` msrv-gate), and documentation.
- **Three-crate product shape.** Behavior work lives in `shipper-core`. CLI work (clap derive, help text, progress rendering) lives in `shipper-cli`. The `shipper` crate is an install façade plus curated re-export; it changes rarely.
- **Events are authoritative.** `events.jsonl` is the source of truth for what happened. `state.json` is a projection. `receipt.json` is a summary derived at end-of-run. When the three disagree, events win — and a drift is a bug.
- **Tokens are opaque strings, never logged.** Token resolution follows `CARGO_REGISTRY_TOKEN` → `CARGO_REGISTRIES_<NAME>_TOKEN` → `$CARGO_HOME/credentials.toml`. The `shipper-output-sanitizer` crate sanitizes cargo and shell output before persistence or logging.
- **Registry-truth reconciliation is stable and must not regress.** Ambiguous `cargo publish` outcomes reconcile against registry truth before Shipper retries or resumes. Reviews of publish-path code should check retry classification, `Published` / `NotPublished` / `StillUnknown` handling, and safe-stop behavior instead of treating ambiguity as a blind retry path.

## CI invariants

- **All product behavior change is gated by `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace`.**
- **MSRV is enforced by `MSRV Check` on PR and by `msrv-gate` on the publish train.** Both pin to the workspace's declared `rust-version`.
- **Tests that mutate environment variables or filesystem are `#[serial]`** via `serial_test` for isolation.
- **Registry interactions in tests use `tiny_http` mock servers, never real registries.**
- **Snapshot tests use `insta`; property-based tests use `proptest`.**
- **CI runs on ubuntu, windows, and macos for the test matrix.** Windows behavior is not optional.

## Pull-request currentness invariants

- A substantive review binds repository, PR number, head SHA, base ref and base SHA, merge-base SHA, synthetic merge/check commit where applicable, review skill/rules identity, and relevant tool/schema/fixture/configuration/receipt identities.
- Head identity alone is insufficient. Base or merge-base movement can change the effective candidate without moving the head.
- Candidate judgment and live integration are separate. Substantive results are `REVIEW_CURRENT`, `CHANGES_REQUIRED`, `NOT_PROVEN`, `BLOCKED_BY_PREREQUISITE`, or `SUPERSEDED_OR_CLOSE`; integration results are `INTEGRATION_READY`, `PR_IN_FLIGHT`, `MERGE_BLOCKED`, or `NOT_PROVEN`.
- Green CI, mergeability, an approval, a bot summary, or zero unresolved threads cannot substitute for substantive review.
- A failed, cancelled, skipped, rate-limited, malformed, placeholder, or unavailable reviewer is unavailable evidence, not a clean review.
- Repairs invalidate the findings and dimensions they affect. Rerun affected proof and re-review affected semantics and repair-created edge cases.
- Reviewer identity alone does not create independence. Record authorship/repair posture, live evidence reloaded, external controls, correlated-failure risk, and author-side versus independent posture.
- Every PR in a stack receives its own candidate judgment before campaign synthesis.
- Valid inline findings receive a reply naming the fixing commit and focused proof, or an evidence-backed rejection, before resolution. Blanket automated thread resolution is forbidden.
- Shared semantics live in `docs/agent-context/review-currentness.md`, while current executable authority remains in `.agents/skills/` and `.claude/skills/`. Retained `.factory/**` material is restoration-only.

## Droid workflow invariants (retired)

The Droid workflows were removed, so none of the invariants below is currently enforced by anything that runs. They are retained as the configuration record for anyone restoring the lane.

One of them is load-bearing and was learned the hard way: the `droid-action-safe` action exchanges a **GitHub OIDC token** for a Factory app token, so any Droid job requires `id-token: write`. Removing that grant as "unused OIDC authority" silently broke every Droid run.

- Droid review uses MiniMax M3 via Factory Droid BYOK.
- Model is `custom:MiniMax-M3-0` for both `review_model` and `security_model`.
- Runtime BYOK settings are written to `$HOME/.factory/settings.json` at job time, and stale `$HOME/.factory/settings.local.json` is removed first so it cannot override M3.
- The settings file is written via a single-quoted heredoc so `${MINIMAX_API_KEY}` remains literal in the file.
- Do not rely on the Droid Action `settings:` input to deliver BYOK custom models.
- Clear `ANTHROPIC_AUTH_TOKEN` to an empty string on Droid action steps.
- Clear `ANTHROPIC_BASE_URL` to an empty string on Droid action steps.
- `show_full_output: false` on every Droid action step.
- `upload_debug_artifacts: false` on every Droid action step.
- Droid action ref is `EffortlessMetrics/droid-action-safe@7c1377ccbacddc95560d1570547a5baa51de01ec`. Do not use `Factory-AI/droid-action` directly for MiniMax BYOK workflows.
- Droid workflows install Bun with `oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6 # v2.2.0` and pass `path_to_bun_executable` into the Droid action so the pinned wrapper skips its nested Node20 setup-bun path.
- `actions/checkout` ref is `actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0`. Droid workflow action refs are immutable SHAs.
- `automatic_review: true` and `automatic_security_review: true` on the auto-review workflow.
- `review_depth: shallow`.
- `cancel-in-progress: false` on the auto-review and security-scan workflows.
- `pull_request` types include `opened`, `synchronize`, `ready_for_review`, `reopened`.
- The auto-review job is guarded by `github.event.pull_request.head.repo.full_name == github.repository` (same-repo guard). Fork PRs are intentionally skipped because secrets must not run on untrusted fork code.
- Automatic Droid review skips bot-authored PRs before any secret-bearing step, including Dependabot and generated security-report PRs. Maintainers can request an on-demand review through the trusted-actor `@droid` workflow when an LLM review is useful.
- Generated `droid/security-report-*` branches are skipped by Droid Auto Review. Scheduled security-scan PRs are already produced by Factory Droid and should be triaged as generated evidence instead of broadening `allowed_bots` for recursive bot review.
- Draft PRs are intentionally reviewable.
- `[skip-review]` in the PR title opts out of automatic review.
- The manual `@droid` workflow is guarded by `OWNER`, `MEMBER`, or `COLLABORATOR` `author_association` on every event branch, plus the same-repo guard on the `pull_request` event branch.
- `MINIMAX_API_KEY` is set at the job-level `env`.
- `FACTORY_API_KEY` is passed as an action input, not exported.
- Scheduled security scan has both `workflow_dispatch` and a `cron: "0 8 * * 1"` (Monday 08:00 UTC) trigger.
- Scheduled scan uses `security_scan_schedule: true`, `security_scan_days: 7`, `security_severity_threshold: medium`, `security_block_on_critical: true`, `security_block_on_high: false`.
- `pull_request_target` is not used anywhere.
- Droid jobs run on explicitly scoped `em-ci-small` self-hosted runners with
  `em-ci`, `cx53`, `rust-large`, and `trusted-pr` labels. PR-triggered paths
  must keep the same-repo guard or the manual `@droid` author-association
  guard so secrets never run on untrusted fork code.
- Raw Droid debug artifact upload is not enabled.
- Raw `$HOME/.factory/**` and `droid-prompts/**` are not uploaded.
- Wrapper-comment post-processing is not added.

## Review output invariants

- No naked `LGTM`. Clean reviews include the reviewed subject, inspected surfaces, challenge passes, existing-finding disposition, residual risk, validation signal, independence posture, and substantive candidate result.
- Findings use the `[P0|P1|P2]` packet format: title, failure mode, why here, fix direction, validation, confidence, and evidence provenance.
- Every claim is marked `Observed:`, `Reported:`, or `Not verified:`.
- Prefer one bounded review with exact-line inline findings; suppress duplicate conversations.
- No `@mentions` of humans, teams, bots, or organizations in Droid-generated content.
- No second-person address. Address the diff, not the author.

## Out of scope for baseline Droid rollout

Until a deliberate update PR lands, the following are explicitly out of scope and should be rejected in review of Droid workflow changes:

- `review_depth: deep`.
- `pull_request_target` triggers.
- Relaxing the Droid runner trust model, same-repo guard, or manual `@droid`
  author-association guard.
- Fork-PR secret execution.
- Wrapper-comment post-processing to strip Factory mentions.
- Untested global permission reductions (e.g., dropping `contents: write` on auto-review without a focused permission-test PR proving the working Factory action still functions).
- Replacing `EffortlessMetrics/droid-action-safe` with `Factory-AI/droid-action` directly while MiniMax BYOK is in use and upstream lacks a debug-artifact disable input.
