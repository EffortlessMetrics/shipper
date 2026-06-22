# Droid Smoke Tests

This file documents how to verify the Factory Droid workflows in shipper after a workflow change merges. Run the full sequence the first time, and the relevant subset after any change to `.github/workflows/droid*.yml`, `.factory/`, or `docs/agent-context/`.

## Prerequisites

Before any smoke test:

- `FACTORY_API_KEY` is set in repo or selected-org secrets.
- `MINIMAX_API_KEY` is set in repo or selected-org secrets.
- Factory Droid GitHub App is installed for the repo.
- A trusted-actor account (`OWNER`, `MEMBER`, or `COLLABORATOR`) is available for manual `@droid` triggers.

## 1. Automatic review

1. Open a same-repo PR. A draft PR is sufficient.
2. Confirm `Droid Auto Review` starts. The workflow should be visible in the PR checks list.
3. Open the job log. Confirm:
   - the `Configure MiniMax BYOK for Factory Droid` step ran;
   - the `Install Bun for Droid action` step ran before the Droid action step;
   - `MINIMAX_API_KEY` appears as `***` (masked), not expanded;
   - the Droid action initializes with `custom:MiniMax-M2.7-0`.
4. Confirm the review output is not a naked `LGTM`.
5. Confirm clean review output includes:
   - inspected surfaces;
   - checks performed;
   - why no comments;
   - residual risk;
   - validation signal with `Observed:`, `Reported:`, `Not verified:` lines.
6. Confirm `[skip-review]` in a PR title prevents the workflow from running.
7. Confirm a branch named `droid/security-report-<date>` does not run the auto-review workflow.
8. Confirm a fork PR (if one is available) does not run the auto-review workflow with secrets.

## 2. Manual review

As a trusted actor, comment on a PR:

```
@droid review
```

Expected:

- the `Droid Tag` workflow runs;
- the trusted-actor guard allows the run;
- the Droid action initializes with `custom:MiniMax-M2.7-0`;
- emitted comments follow the `[P0|P1|P2]` repair-queue format;
- no `@mentions` of humans, teams, bots, or organizations appear in the output.

If a non-trusted actor (e.g., a first-time contributor with `author_association` `NONE` or `CONTRIBUTOR`) posts `@droid review`, the workflow should be skipped by its `if:` guard.

## 3. Manual security review

As a trusted actor, comment on a PR:

```
@droid security
```

Expected:

- the `Droid Tag` workflow runs;
- security review runs against the diff;
- no unrelated code edits are produced;
- findings include severity and `Fix direction:`;
- output does not contain expanded provider keys, GitHub tokens, or authorization headers.

## 4. Full security scan

Trigger `Droid Security Scan` once via the Actions UI (`workflow_dispatch`).

Expected:

- the model is `custom:MiniMax-M2.7-0`;
- the scan window is 7 days (`security_scan_days: 7`);
- the threshold is `medium`;
- critical findings would block (`security_block_on_critical: true`);
- high findings do not block (`security_block_on_high: false`);
- no secrets appear in logs;
- no raw `droid-review-debug-<run_id>` artifact is uploaded.

The scheduled trigger (`cron: "0 8 * * 1"`, Monday 08:00 UTC) does not need to be smoke-tested manually; it is exercised on its natural schedule.

If the scan opens a `droid/security-report-<date>` PR, triage the generated
report directly. That PR is intentionally excluded from Droid Auto Review so
the bot allowlist can stay limited to Dependabot.

## 5. Artifact and log hygiene

After any Droid run:

1. Open the run summary page.
2. Confirm the artifacts list does not contain `droid-review-debug-<run_id>` or any raw Droid debug artifact.
3. Spot-check the job log for:
   - no expanded `MINIMAX_API_KEY` value;
   - no `Authorization: Bearer ...` header lines;
   - no contents of `$HOME/.factory/settings.local.json` with the key expanded;
   - no raw prompt files leaked into the log.
4. Confirm the Droid action step does not emit an `oven-sh/setup-bun` Node20 deprecation annotation.

If any secret, authorization header, settings file, or raw prompt content appears, treat it as a security incident and open a tracking issue. Do not redact in place; rotate `MINIMAX_API_KEY` and `FACTORY_API_KEY` first.

## 6. Failure modes to watch for

- `FACTORY_API_KEY is required to run Droid Exec` — the repo or org secret is missing or not scoped to this repo.
- `400 Bad Request` from MiniMax — the heredoc may have expanded `${MINIMAX_API_KEY}` at shell time; confirm `cat > settings.local.json <<'JSON'` is single-quoted so the variable stays literal in the file.
- `model not found` — verify `review_model` and `security_model` are both `custom:MiniMax-M2.7-0` and that the `customModels` block in `settings.local.json` matches.
- Auto-review running on a fork PR — verify the same-repo guard `github.event.pull_request.head.repo.full_name == github.repository` is present and not commented out.
- Manual `@droid` running for a non-trusted actor — verify the `author_association` guard on every event branch in `droid.yml`.
- Node20 deprecation warnings from Droid's internal Bun setup — verify the workflow's pinned `Install Bun for Droid action` step still has `id: setup-bun`, still produces `steps.setup-bun.outputs.bun-path`, and the Droid action step still passes that path via `path_to_bun_executable`.
- `cancel-in-progress: true` accidentally introduced — should be `false` so active reviews are not interrupted by new pushes.
