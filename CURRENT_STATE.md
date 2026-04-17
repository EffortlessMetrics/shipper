# CURRENT_STATE — Shipper truth board

**Built from:** `origin/main` @ `5beab9b` (ci: isolate CARGO_HOME in preflight_allow_dirty_snapshot, #86)
**Date:** 2026-04-16
**Purpose:** Phase 0 truth sync per `shipper_full_completion_plan_for_claude.md`.

---

## Workspace shape

- **Crate count (workspace members):** 12
- **Public (publishable) crates (12/12):**
  - `shipper-types`
  - `shipper-duration`
  - `shipper-cargo-failure`
  - `shipper-output-sanitizer`
  - `shipper-encrypt`
  - `shipper-retry`
  - `shipper-sparse-index`
  - `shipper-registry`
  - `shipper-config`
  - `shipper-webhook`
  - `shipper` (core library)
  - `shipper-cli` (binary)

No `publish = false` crates in the workspace — every member is public surface.

---

## Shipper-driven release workflow

**Status:** Merged. Present at `.github/workflows/release.yml`.

- Dogfoods itself: `shipper plan` → `shipper preflight` → `shipper publish` → `shipper resume`.
- Triggers: `v*.*.*` tag push (full release), `workflow_dispatch` with `mode=rehearse` (plan + preflight dry-run only), `workflow_dispatch` with `mode=resume` (downloads `.shipper/` artifact, resumes).
- Binary-artifact + GitHub Release steps finalize **after** crates.io publish succeeds.

---

## CI lane status on main

Most recent `CI` workflow run on `main` (run `24491433099`):

| Lane | Status |
|---|---|
| Lint (fmt + clippy) | ✅ success |
| MSRV Check | ✅ success |
| Tests (nextest) — ubuntu-latest | ✅ success |
| Tests (nextest) — windows-latest | ✅ success |
| Tests (nextest) — macos-latest | ✅ success |
| Security Audit | ✅ success |
| Documentation | ✅ success |
| Coverage (llvm-cov) | ✅ success |
| BDD Tests | ✅ success |
| Build — ubuntu-latest x86_64 | ✅ success |
| Build — ubuntu-latest aarch64 | ✅ success |
| Build — windows-latest x86_64 | ✅ success |
| Build — macos-latest x86_64 | ✅ success |
| Build — macos-latest aarch64 | ✅ success |
| Release Build | ✅ success |
| Fuzz Smoke (PR) | ⚪ skipped (PR-only gate) |

**Separate workflows on main:**
- `Fuzz` (workflow `fuzz.yml`): ✅ latest 5 runs all green.
- `architecture-guard`: ❌ **latest 5 runs all failed** — but see caveat below.

### architecture-guard — fix landed but unverified

- Root cause of past failures: grep matched forbidden-import examples inside `crates/shipper/src/ops/CLAUDE.md` (the CLAUDE.md documents the rule with `❌ use crate::engine::...` lines, which the guard scanned as real violations).
- **Fix merged in #85 (`ea75b53`):** added `--include='*.rs'` to every guard grep. Current `.github/workflows/architecture-guard.yml` on main is correct.
- **Why it still shows red in the runs list:** the guard's `paths:` filter restricts triggering to `crates/shipper/src/**`. No commit on main since #85 has touched that path — #86 only touched `crates/shipper-cli/tests/`. So the fixed guard has not had an opportunity to re-run and post a green status.
- **Risk:** low-confidence green. The fix is in place but unverified until the next commit that touches `crates/shipper/src/**` triggers it.

### Red lanes on main: **none** (modulo the unverified architecture-guard)

---

## Package truth

`cargo package --list -p <crate> --allow-dirty` ran against every public crate on `origin/main`. All 12 succeeded and emitted file lists (`--allow-dirty` used only because the working tree has the untracked `.claude/` dir; does not affect package contents).

- ✅ `shipper-types`
- ✅ `shipper-duration`
- ✅ `shipper-cargo-failure`
- ✅ `shipper-output-sanitizer`
- ✅ `shipper-encrypt`
- ✅ `shipper-retry`
- ✅ `shipper-sparse-index`
- ✅ `shipper-registry`
- ✅ `shipper-config`
- ✅ `shipper-webhook`
- ✅ `shipper`
- ✅ `shipper-cli`

**Not yet rerun in this truth sync:** `cargo publish --dry-run -p <crate>` for the 7 leaf crates. The plan calls this a Phase 3 revalidation. Held until the Phase 0/1 decision gate.

---

## Rehearsal status

- **`.shipper/` directory on main:** absent. No persisted plan/receipt/events from a prior local rehearsal are checked in.
- **Release workflow has not been exercised** in `rehearse` mode against `origin/main` in this sync (not attempted as part of Phase 0 — it's a Phase 4 action).

---

## Open PRs

| # | Title | Author |
|---|---|---|
| 87 | ci: fast-path shipper-encrypt proptests on PR matrix | EffortlessSteven (this session) |
| 46 | deps(deps): bump clap_complete from 4.5.66 to 4.6.1 | dependabot |
| 45 | ci(deps): bump softprops/action-gh-release from 2 to 3 | dependabot |
| 41 | deps(deps): bump sha2 from 0.10.9 to 0.11.0 | dependabot |
| 40 | deps(deps): bump hmac from 0.12.1 to 0.13.0 | dependabot |
| 39 | ci(deps): bump codecov/codecov-action from 5 to 6 | dependabot |
| 31 | deps(deps): bump which from 8.0.0 to 8.0.2 | dependabot |
| 30 | deps(deps): bump tokio from 1.49.0 to 1.50.0 | dependabot |

All non-#87 PRs are dependabot; per plan's "what NOT to do yet", holding the `which` / `clap_complete` / `tokio` / `sha2` / `hmac` bundle until CI is boring. CI is now boring, but the plan deprioritizes this relative to product milestones.

---

## Decision gate

Per plan:

> **Path A:** if the hardening PRs are already merged and main CI is green, skip directly to **Phase 3**.
> **Path B:** if docs / nextest / fuzz / audit are still red, execute **Phase 1 and Phase 2** first.

**Recommendation: Path A**, with one caveat.

Justification:
- docs ✅, nextest ✅ (3 OSes), fuzz (separate workflow) ✅, audit ✅, all other CI lanes ✅, all 12 `cargo package --list` ✅, release workflow merged, Phase 1 hardening is either already landed or not needed.
- Caveat: architecture-guard is structurally green (workflow file contains the fix) but has not re-posted a success on main due to the paths-filter trigger. This is low-risk but worth forcing a re-run before declaring Phase 0 fully satisfied.

### Suggested Phase 3 entry actions

1. Force architecture-guard to re-run on `main` once (via `workflow_dispatch` if the workflow allows it, or by any small no-op commit touching `crates/shipper/src/**`).
2. Rerun `cargo publish --dry-run` across the 7 leaf crates to confirm the sibling-on-crates.io dry-run pattern still matches.
3. Then move to Phase 4 rehearsal (`shipper plan` / `preflight` / `publish` / `resume`) against a non-production target.
4. Only after a clean rehearsal: Phase 5 (runbook + first real crates.io train).

### Non-blocking side artifacts from this sync

- PR #87 (CI proptest case reduction) is open and orthogonal; merge or close independently of Phase 3. Does not gate the release path.
