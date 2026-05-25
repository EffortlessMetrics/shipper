# Review Invariants

This file captures invariants for both human and Droid review of shipper PRs. These are durable expectations a reviewer can rely on regardless of which lens they apply.

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

## Droid workflow invariants

These constrain how Factory Droid review is configured for shipper. A reviewer should reject any change that violates them without an explicit, scoped justification PR.

- Droid review uses MiniMax M2.7 via Factory Droid BYOK.
- Model is `custom:MiniMax-M2.7-0` for both `review_model` and `security_model`.
- Runtime BYOK settings are written to `$HOME/.factory/settings.local.json` at job time.
- The settings file is written via a single-quoted heredoc so `${MINIMAX_API_KEY}` remains literal in the file.
- Do not rely on the Droid Action `settings:` input to deliver BYOK custom models.
- Do not set `ANTHROPIC_AUTH_TOKEN`.
- Do not set `ANTHROPIC_BASE_URL`.
- `show_full_output: false` on every Droid action step.
- `upload_debug_artifacts: false` on every Droid action step.
- Droid action ref is `EffortlessMetrics/droid-action-safe@7c1377ccbacddc95560d1570547a5baa51de01ec`. Do not use `Factory-AI/droid-action` directly for MiniMax BYOK workflows.
- Droid workflows install Bun with `oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6 # v2.2.0` and pass `path_to_bun_executable` into the Droid action so the pinned wrapper skips its nested Node20 setup-bun path.
- `actions/checkout` ref is `actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2`. Droid workflow action refs are immutable SHAs.
- `automatic_review: true` and `automatic_security_review: true` on the auto-review workflow.
- `review_depth: shallow`.
- `cancel-in-progress: false` on the auto-review and security-scan workflows.
- `pull_request` types include `opened`, `synchronize`, `ready_for_review`, `reopened`.
- The auto-review job is guarded by `github.event.pull_request.head.repo.full_name == github.repository` (same-repo guard). Fork PRs are intentionally skipped because secrets must not run on untrusted fork code.
- `allowed_bots: dependabot[bot]` is set so Dependabot dependency-bump PRs receive Droid Auto Review. The safe action rejects non-human actors by default; this list narrowly re-permits Dependabot. Do not change this to `'*'`. Adding additional bots requires an explicit follow-up PR with justification.
- Draft PRs are intentionally reviewable.
- `[skip-review]` in the PR title opts out of automatic review.
- The manual `@droid` workflow is guarded by `OWNER`, `MEMBER`, or `COLLABORATOR` `author_association` on every event branch, plus the same-repo guard on the `pull_request` event branch.
- `MINIMAX_API_KEY` is set at the job-level `env`.
- `FACTORY_API_KEY` is passed as an action input, not exported.
- Scheduled security scan has both `workflow_dispatch` and a `cron: "0 8 * * 1"` (Monday 08:00 UTC) trigger.
- Scheduled scan uses `security_scan_schedule: true`, `security_scan_days: 7`, `security_severity_threshold: medium`, `security_block_on_critical: true`, `security_block_on_high: false`.
- `pull_request_target` is not used anywhere.
- Droid jobs run on `self-hosted` runners. PR-triggered paths must keep the
  same-repo guard or the manual `@droid` author-association guard so secrets
  never run on untrusted fork code.
- Raw Droid debug artifact upload is not enabled.
- Raw `$HOME/.factory/**` and `droid-prompts/**` are not uploaded.
- Wrapper-comment post-processing is not added.

## Review output invariants

- No naked `LGTM`. Clean reviews include an inspection record: inspected surfaces, checks performed, why no comments, residual risk, validation signal.
- Findings use the `[P0|P1|P2]` packet format: title, failure mode, why here, fix direction, validation, confidence.
- Every claim is marked `Observed:`, `Reported:`, or `Not verified:`.
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
