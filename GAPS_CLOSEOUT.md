# Gaps closeout audit — against `main` as of 2026-04-18

Three-column audit for #90, #97, #98 per their issue checklists. Single
source of truth for "what's done vs. still missing" before merging #135
and #122. Produced by reading the issues directly against the merged
commits on `main`.

---

## #90 Recover — CLOSED ✅

Issue acceptance checklist:

| Item | Status | Where |
|---|---|---|
| Rehearsal procedure executable end-to-end | ✅ on `main` | `docs/how-to/run-recover-rehearsal.md` (PR #124) |
| State correctly persists between interruption and resume | ✅ on `main` | Regression test in `crates/shipper-cli/tests/e2e_rehearse.rs` (PR #124); fix for #125 silent skip in PR #130 |
| Plan-ID guard rejects resume if workspace changed | ✅ on `main` | `engine::mod.rs` resume plan_id check (pre-existing, verified by PR #124 test) |
| Resume skips already-published packages | ✅ on `main` | Verified by `bdd_resume` suite + PR #124 synthetic |
| Resume continues from first Pending package | ✅ on `main` | Covered by existing BDD tests |
| All crates visible on crates.io after resume | 🟡 **operator-side** | Playbook in `docs/how-to/run-recover-rehearsal.md`; real rehearsal run is operator action, not a PR gate |

**Follow-up filed and resolved this cycle:**
- #125 silent-skip in `events.jsonl` → PR #130 merged
- #126 Failed→Skipped state drift → PR #130 regression test passes (current code handles correctly)

**Still operator-side, not missing from code:**
- A live throwaway-tag run with a real crates.io-backed workflow cancellation. Playbook is ready; artifact-collection + pass/fail rubric documented. Code side is done.

**Audit verdict: closed honestly.** No code gap.

---

## #97 Prove tier 2 — OPEN, 3/4 implemented

Issue proposes a two-phase preflight. Phase 1 (existing) unchanged.
Phase 2 (new) is 4 steps:

| Phase 2 step | Status | Where / gap |
|---|---|---|
| 1. Publish packaged tarballs to a configured alt registry | ✅ on `main` | `engine::run_rehearsal` (PR #127) — invokes `cargo publish --registry <rehearsal>` per crate in topo order |
| 2. Verify presence on rehearsal registry (API or sparse index) | ✅ on `main` | Post-publish `rehearsal_client.version_exists` (PR #127) |
| 3. Install/build from rehearsal registry (`cargo install --registry <rehearsal>` OR consumer `cargo build`) | ❌ **NOT on `main`** | **This is the gap.** No install-smoke / consumer-build step exists. `shipper rehearse` publishes + verifies visibility, but does not prove the crate is resolvable via the registry index in a real consumer build — which is literally the scenario that killed the rc.1 first-publish. |
| 4. Record proof in `RehearsalComplete` event | ✅ on `main` | PR #127 emits `RehearsalComplete { passed, registry, plan_id, summary }` |

Hard gate (issue's "Hard gate" section):

| Item | Status | Where |
|---|---|---|
| Live publish refuses without passing rehearsal for same `plan_id` | ✅ on `main` | `engine::enforce_rehearsal_gate` (PR #133) |
| `--skip-rehearsal` override with loud warning | ✅ on `main` | Same gate, rule 2 |
| `rehearsal.json` sidecar persisting outcome | ✅ on `main` | `state::rehearsal` module (PR #133) |
| Plan-ID binding of rehearsal outcome | ✅ on `main` | `RehearsalComplete.plan_id` + sidecar check (PR #133) |

Config surface:

| Item | Status | Where |
|---|---|---|
| `[rehearsal]` TOML section | ✅ on `main` | `shipper-config` (PR #120) |
| `--rehearsal-registry <name>` flag | ✅ on `main` | PR #120 |
| `--skip-rehearsal` flag | ✅ on `main` | PR #120 |

**The one real gap:** install/smoke check. **Needs a narrow follow-up PR**: `#97 PR 4` — wire a `cargo install --registry <rehearsal>` step (or a tiny consumer-workspace `cargo build`) after publish + visibility, emit `RehearsalSmokeCheckSucceeded` / `...Failed` events, fail the rehearsal on failure.

**Audit verdict:** 85% done. One narrow follow-up closes it.

---

## #98 Remediate — OPEN, mixed

Issue proposes 4 features. Grading each:

| Feature | Status | Gap |
|---|---|---|
| Receipt fields (`compromised_at`, `compromised_by`, `superseded_by`) | ✅ on `main` | `shipper-types` PackageReceipt (PR #132) |
| `shipper plan-yank --from-receipt <path> --starting-crate <name> --reason <text>` | 🟡 **partial** | Implemented `--from-receipt` + `--compromised-only` (PR #132). **Missing `--starting-crate <name>` + `--reason <text>`.** These are different semantics: the issue wants graph-based "crate X is broken; who depends on it?" reasoning; we ship receipt-filter "which packages carry a compromised_at marker?" reasoning. Both useful; only the latter landed. |
| `shipper yank --plan <plan-id>` (execution wrapper) | ❌ **NOT on `main`** | Our `shipper yank --crate <N> --version <V>` yanks one crate (PR #121). The issue asks for a **plan executor** that takes a saved yank plan and walks it. Missing. |
| `shipper fix-forward --plan <plan-id>` (execution) | ❌ **NOT on `main`** | Our `shipper fix-forward` is planning-only (PR #134). The issue asks for an **executor** that runs fix-forward end-to-end. Missing. |

Acceptance criteria:

| Item | Status | Notes |
|---|---|---|
| Maintainer loads receipt, names broken crate, gets working yank plan without manual dep analysis | 🟡 partial | Works via `--compromised-only` after operator annotates receipt with `yank --mark-compromised`. Does NOT work via `--starting-crate <name>` (not implemented). |
| `yank` and `fix-forward` execute safely with state resumability | ❌ not on `main` | Both are planning-only in the current code. |
| Docs distinguish yank semantics; highlight re-pinning | ✅ on `main` | `docs/how-to/remediate-a-compromised-release.md` (PR #134) |

**Real gaps (need follow-up PRs):**

1. `#98 PR 4` — `shipper plan-yank --starting-crate <name>`. Graph-based yank planner: given a broken crate name, walk the dependency edges in the receipt (via the plan's `dependencies` map) to find dependents; emit reverse-topo plan. Complements the existing `--compromised-only` filter. Small (~100 LOC + tests).
2. `#98 PR 5` — plan execution. Take a saved yank plan (or fix-forward plan) and run it step-by-step with state resumability. Wraps the existing `cargo_yank` and `cargo_publish` primitives. Larger (~200 LOC + tests + resumable state for yank runs).

**Audit verdict:** 60% done. Two narrow follow-ups needed.

---

## Open PRs against this audit

| PR | Status | Audit verdict |
|---|---|---|
| **#135** Packaging unification (reframed #95) | CI green, ready | Independent of #97/#98 closeout. Safe to merge now. |
| **#122** Trusted Publishing (#96) | Open, rebased | See review concerns below. Hold for fixes. |

### #122 review concerns (raised by 3rd-party review)

1. **Mixed-registration fallback.** `${{ steps.auth.outputs.token \|\| secrets.CARGO_REGISTRY_TOKEN }}` falls back ONLY if the OIDC action outputs an empty token. If the action succeeds but only some of the 12 crates are registered as trusted publishers, `cargo publish` 401s mid-train with no graceful retry on the long-lived token. **Fix**: either (a) add a preflight that probes each crate with the minted OIDC token and fails the whole run early if any crate 401s, or (b) document explicitly that ALL 12 crates must be registered before enabling Trusted Publishing and keep the secret as the bootstrap path.
2. **Rehearsal auth scope guard.** The rehearse job uses `continue-on-error: true` on the auth mint step, but doesn't set `environment: release`. That means the OIDC token minted for rehearsal may scope under a different environment than the production publish job's token — silently hiding a real Trusted Publishing misconfiguration during rehearsal. **Fix**: either add `environment: release` to the rehearse job too, or document explicitly that rehearsal validates only the mechanism, not the full production scope binding.

**Audit verdict for #122:** Content correct, two operational concerns. Reworkable with ~20 lines of workflow change + a doc paragraph.

---

## Recommended merge / follow-up order

### Right now
1. **Merge #135** (package unification) — independent of remaining pillar gaps; CI green; waited for review concerns are design-captured in the PR body.

### Immediately after #135
2. **Fix #122's two review concerns** (mixed-registration early-fail + rehearsal environment binding), then merge.

### Narrow follow-ups to close pillar gaps
3. **#97 PR 4**: install/smoke check against rehearsal registry — fully closes #97.
4. **#98 PR 4**: `plan-yank --starting-crate <name>` graph-based planner — part one of closing #98.
5. **#98 PR 5**: plan execution wrappers for yank + fix-forward — closes #98 acceptance criteria 2/3 (execute with resumability).

### Tracker cleanup
6. Close #97 once PR 4 lands.
7. Close #98 once PR 4 + PR 5 land.

### Parked (do NOT unpin yet)
- 4 Dependabot PRs — batch after the pillar closeout.
- Docs/examples pass (demo workspace, example artifacts) — after #135 + #122 land so the new install/auth story is what docs describe.

---

## The blunt answer

Packaging UX (#135) is independent and ready. Land it.

Two tiny review fixes close #122.

**#97 is one narrow PR away from done.** **#98 is two narrow PRs away from done.** Neither blocks #135.

Issue trackers lag reality. After the above PRs land, close #97 and #98 explicitly with a comment pointing at the commits that closed each acceptance criterion.
