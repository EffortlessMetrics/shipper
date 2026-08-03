# Security Scan Report

**Generated:** 2026-07-27
**Scan Type:** Weekly Scheduled
**Repository:** EffortlessMetrics/shipper-swarm
**Severity Threshold:** medium

## Executive Summary

| Severity | Count | Auto-fixed | Manual Required |
|----------|-------|------------|-----------------|
| CRITICAL | 0     | 0          | 0               |
| HIGH     | 0     | 0          | 0               |
| MEDIUM   | 0     | 0          | 0               |
| LOW      | 0     | 0          | 0               |

**Total Findings:** 0
**Auto-fixed:** 0
**Manual Review Required:** 0

The weekly scan of `droid/security-report-2026-07-27` over the last 7
days (2026-07-20 through 2026-07-27, UTC) examined the single commit
that landed on `main` in the window: `a81869e deps(deps): bump regex
from 1.13.0 to 1.13.1 (#187)` by `dependabot[bot]`. The commit is a
Dependabot-driven bump of `regex` (1.12.4 → 1.13.1), `regex-automata`
(0.4.14 → 0.4.16), `anyhow` (1.0.103 → 1.0.104), and `indicatif`
(0.18.5 → 0.18.6) in `Cargo.lock`, plus the engine refactor that ships
on top of it. Viewed against the previous scan's reference commit
(`0589541`), the functional delta is 40 files / +3,072 insertions /
−1,896 deletions.

The functional changes inside the engine crate are concentrated in
two new files:

- `crates/shipper-core/src/engine/execute_package.rs` — the canonical
  per-package executor (`publish_package`, `run_sequential_scheduler`,
  the `commit_*_transition` shims, the `emit_retry_backoff`,
  `record_rate_limit_observed`, `flush_event_log`,
  `record_readiness_event`, `wait_after_retry`, and
  `write_reconciliation_report_best_effort` helpers).
- `crates/shipper-core/src/engine/parallel/scheduler.rs` — the
  dependency-level concurrency scheduler that consumes
  `execute_package::publish_package` to publish one level at a time.

Plus the renaming / split of the former
`crates/shipper-core/src/engine/parallel/publish.rs` into
`execute_package.rs` (the executor) and `parallel/scheduler.rs` (the
parallel scheduler), the addition of a new `EventType::PackageUploaded`
event variant in `shipper-types`, the matching `state::rebuild::apply_event`
arm in `shipper-core/src/state/rebuild.rs`, the new tests in
`crates/shipper-core/src/engine/parallel/tests.rs`, and the workflow
+ helper-script changes in `.github/workflows/em-ci-routed-rust.yml`,
`.github/workflows/runner-routing-guard.yml`, and the newly extracted
`scripts/ci/normalize-routed-rust-result.py`. Visibility tightens
(`pub(super)` → `pub(crate)`) but no callers are widened beyond the
engine crate.

No application code in an auth, token-resolution, encryption,
state-persistence, or subprocess-invocation path was altered in a way
that introduces a finding at the configured `medium` severity
threshold. The subprocess boundary is unchanged: `execute_package.rs`
delegates every `cargo` invocation to `crate::ops::cargo::cargo_publish`,
which routes through `process::run_command_with_timeout` and applies
`shipper_output_sanitizer::redact_sensitive` to stdout/stderr tails
before they reach `events.jsonl`, `state.json`, or `receipt.json`.
The events-as-truth invariant is preserved: every state-changing
transition goes through `crate::engine::transition::commit_*`, and
the new `PackageUploaded` event is the durable checkpoint that
replaces (and remains backward-compatible with) the prior
`ReadinessStarted` checkpoint for `PackageState::Uploaded`.

The CI-workflow changes are net-hardening, not net-loosening:

- The `pull_request` trigger set in `em-ci-routed-rust.yml` shrinks
  from `[opened, synchronize, reopened, labeled]` to
  `[opened, synchronize, reopened]`. Label additions no longer
  re-fire the required workflow, which closes the latent
  label-trigger spoofing window in the route-fallback decision.
- The `route` job's `if:` guard stops consulting
  `github.event.label.name`, so an `allow-github-hosted` /
  `ci-budget-ack` label applied by any user with write access can no
  longer re-route a human PR to GitHub-hosted runners mid-flight.
- The inline Python block under `rust_small_normalize` is replaced
  by `python3 scripts/ci/normalize-routed-rust-result.py`, which is
  now also reachable from the lint workflow via `--test`, and which
  has its own `--test` self-test (passes locally: `routed result
  normalization tests passed`).
- `runner-routing-guard.yml` now installs actionlint with a SHA-256
  pin (`ACTIONLINT_VERSION=1.7.12`,
  `ACTIONLINT_SHA256=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8`)
  before validating workflow syntax, then runs
  `actionlint -shellcheck=""` against every workflow in the tree.

The repository remains in a strong security posture: `unsafe_code =
"forbid"` is enforced workspace-wide, every event/state/receipt
triplet is preserved, the output sanitizer redacts tokens before
persistence, the encryption crate uses AES-256-GCM with PBKDF2
(100,000 iterations) and per-call random salt + nonce, the registry
HTTP client enforces timeouts, and the fuzzer corpus
(`fuzz/fuzz_targets/`) covers token resolution, encryption, output
redaction, plan building, retry, and webhook payload construction.

## Critical Findings

None.

## High Findings

None.

## Medium Findings

None.

## Low Findings

None.

## Observations (Below Severity Threshold, Not Reported as Findings)

These are not findings under the configured `medium` threshold. They
are recorded for the next weekly scan and for engineering awareness;
no remediation is required for this report.

### OBS-1: Floating action tags in CI workflows (carried-over OM-2)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Tampering (supply chain) |
| **CWE** | CWE-1357 (Reliance on Untrusted Component) |
| **Files** | `.github/workflows/*.yml` (all 14 workflows) |
| **Status** | Accepted risk, tracked under OM-2 in `.factory/threat-model.md` |

**Description:**
Workflow references to third-party GitHub Actions use floating major
tags rather than commit SHAs. Examples observed across the scan
window: `actions/checkout@v7.0.0`, `dtolnay/rust-toolchain@stable`,
`dtolnay/rust-toolchain@nightly`, `dtolnay/rust-toolchain@v1`,
`taiki-e/install-action@v2`, `taiki-e/install-action@cargo-audit`,
`actions/cache@v6`, `actions/upload-artifact@v7`,
`actions/download-artifact@v8`, `codecov/codecov-action@v7`,
`softprops/action-gh-release@v3`,
`rust-lang/crates-io-auth-action@v1`. The Droid-related actions
(`EffortlessMetrics/droid-action-safe@7c1377c...`,
`oven-sh/setup-bun@0c5077e...`) and the direct `actions/checkout`
reference in the Droid workflows are SHA-pinned. This scan window
adds no new floating-tag references and adds a SHA-256-pinned
`actionlint` install (v1.7.12) to
`.github/workflows/runner-routing-guard.yml`.

**Risk:** A compromise of an upstream major tag could push arbitrary
code into CI. Most jobs hold `contents: read` and run on self-hosted
runners with fork-PR guards. The release workflow (`release.yml`)
uses `rust-lang/crates-io-auth-action@v1` to exchange an OIDC token
for a short-lived crates.io token, but that path is gated by
`if: github.repository == 'EffortlessMetrics/shipper' && github.event_name == 'push'`
and therefore inert in `shipper-swarm` (the dev repo).

**Mitigation already in place:**
- Dependabot is configured (`dependabot.yml` -> `github-actions`
  ecosystem) to bump Actions weekly against this repo.
- The release workflow is gated to the release-authority repo
  (`EffortlessMetrics/shipper`).
- Fork-PR guards are added to every self-hosted job; untrusted fork
  PRs cannot trigger job execution with secrets.
- The runner-routing guard (`runner-routing-guard.yml` +
  `scripts/ci/no-bare-self-hosted.sh`) rejects bare
  `runs-on: self-hosted` declarations, restricting where these jobs
  can land.
- Per-job `permissions:` blocks scope each job to the least privilege
  it needs (most jobs hold `contents: read`).
- The required CI workflow's `pull_request` types have been narrowed
  this scan window to `[opened, synchronize, reopened]`, removing
  the `labeled` trigger that could otherwise be used to re-fire the
  workflow after the fact.

**Recommended Hardening (Optional):**
Pin all third-party Actions to commit SHAs (mirror the pattern
already used for `EffortlessMetrics/droid-action-safe` and
`oven-sh/setup-bun`). Dependabot's group updates will continue to
bump the SHA in lockstep. Trade-off: reduced upstream agility for
tighter supply-chain posture. Not required for this scan window.

### OBS-2: Whitespace-only token values are treated as valid credentials (carried-forward)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Spoofing / Tampering (low impact) |
| **CWE** | CWE-20 (Improper Input Validation), informational only |
| **File** | `crates/shipper-core/src/ops/auth/resolver.rs:88-97` |
| **Status** | Matches cargo behaviour; no fix recommended |

**Description:**
The `resolve_token` implementation treats an env var whose value is
a whitespace-only string as a valid token and returns it as the
resolved credential (the empty-check is `!token.is_empty()`, not
`!token.trim().is_empty()`). This behaviour is pinned by the
`resolve_token_whitespace_only_env_is_not_skipped` test in the same
file. There is no upstream remediation because cargo itself preserves
whitespace-only tokens through `cargo login` and through the
env-var precedence chain.

**Risk:** minimal. A user who explicitly sets
`CARGO_REGISTRY_TOKEN="   "` in their environment gets a whitespace
token; the only consequence is that any subsequent publish attempt
will be rejected by crates.io with HTTP 403, which the existing
`ErrorClass::Permanent` classifier surfaces to the user. No token
is leaked, no privilege escalation is possible, and no other
caller is reachable from this state.

**Recommended action:** none. Documented for future auditability.

### OBS-3: Output sanitizer OSC without terminator consumes to EOF (carried-over OM-3)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Denial of Service (bounded) |
| **CWE** | CWE-400 (Uncontrolled Resource Consumption), informational only |
| **File** | `crates/shipper-output-sanitizer/src/lib.rs` (`strip_ansi`) |
| **Status** | Accepted behaviour, tracked under OM-3 in `.factory/threat-model.md` |

**Description:** `strip_ansi` consumes an unterminated OSC sequence
(`\x1b]...`) to EOF, including any trailing newline. The behaviour
is pinned by `osc_without_terminator_consumes_to_eof` and
`osc_without_terminator_does_not_panic_with_following_lines` tests
in the same file. There is no security impact (no panic, no
out-of-memory condition, no leaked token), but evidence truncation
could cause operator confusion during incident review.

**Recommended action:** none. Documented under OM-3.

### OBS-4: `mask_token` exposes first and last four characters (carried-forward)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Information Disclosure (bounded) |
| **CWE** | CWE-200 (Exposure of Sensitive Information), informational only |
| **File** | `crates/shipper-core/src/ops/auth/resolver.rs:160-167` |
| **Status** | Standard display-masking pattern; no fix recommended |

**Description:** For tokens longer than 8 characters, `mask_token`
returns `<first 4>****<last 4>`. Tokens of 8 characters or fewer are
fully masked. This is the standard display-masking pattern used by
cargo, gh, and other registry CLIs.

**Risk:** minimal. The exposed prefix/suffix carries at most 8 ASCII
characters of token entropy, which is insufficient to recover a
crates.io style token. Display-masked tokens are not usable for
publishing.

**Recommended action:** none.

### OBS-5: Commit-change attack surface is concentrated in engine internals (this scan window)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Repudiation (strengthened, not weakened) |
| **CWE** | CWE-778 (Insufficient Logging), informational only |
| **Files** | `crates/shipper-core/src/engine/execute_package.rs`, `crates/shipper-core/src/engine/parallel/scheduler.rs`, `crates/shipper-core/src/state/rebuild.rs`, `crates/shipper-types/src/lib.rs` |
| **Status** | Net defensive hardening of events-as-truth invariant |

**Description:**
The functional delta in the engine crate is the extraction of
`publish_package` and the `commit_*_transition` shims from the
former `parallel/publish.rs` into a new top-level
`engine::execute_package` module, plus the addition of a parallel
`engine::parallel::scheduler::run_publish_level` that consumes the
same `publish_package`. The new `EventType::PackageUploaded`
variant in `shipper-types::PublishEvent` is the durable checkpoint
that replaces (and remains backward-compatible with) the prior
`ReadinessStarted` checkpoint for `PackageState::Uploaded`, with
matching arms in `state::rebuild::apply_event` for both event
variants and a new `rebuild_package_uploaded_projects_uploaded_until_published`
test that pins the rebuild behavior.

These changes strengthen the events-as-truth invariant rather than
weakening it: a successful terminal transition cannot be written
without its matching attempt timeline (`AttemptDetail` is still
persisted via `commit_attempt_detail_pending` /
`commit_pending_with_attempt_detail` /
`commit_with_attempt_detail` from the prior scan), and the new
`PackageUploaded` event is matched 1:1 to `PackageState::Uploaded`
on rebuild. The existing `redact_sensitive` path through
`ops::cargo::tail_lines` -> `cargo_publish::stdout_tail` /
`stderr_tail` -> `classify_cargo_failure` ->
`AttemptDetail::redacted_message` remains unchanged, so token
redaction continues to flow correctly into persisted attempt detail
and into the new `PackageUploaded` event's transition context.

The `execute_package.rs::commit_*_transition` shims call
`crate::engine::transition::commit_*` with both the event log write
and the state projection write sequenced through the same boundary,
so a partial failure (one write succeeds, the other fails) surfaces
the drift rather than silently advancing the projection. This
preserves the repudiation guarantee that `state.json` cannot lead
`events.jsonl`.

**Recommended action:** none. Track the remaining
`Full AttemptDetail replay from events` follow-up that the prior
commit message explicitly deferred.

### OBS-6: Dependabot supply-chain bump scope (informational)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Not applicable (informational) |
| **CWE** | Not applicable |
| **Files** | `Cargo.lock` (`regex`, `regex-automata`, `anyhow`, `indicatif`) |
| **Status** | Informational only; scans the supply-chain bumps Dependabot shipped this week |

**Description:**
The single commit in scope (`a81869e deps(deps): bump regex from
1.13.0 to 1.13.1 (#187)`) bumps four transitive dependencies via
Dependabot. The Cargo.lock transitions observed in
`git diff 0589541..a81869e -- Cargo.lock` are:

| Crate           | From    | To      | Bump kind |
|-----------------|---------|---------|-----------|
| `regex`         | 1.12.4  | 1.13.1  | minor + patch |
| `regex-automata`| 0.4.14  | 0.4.16  | minor + patch |
| `anyhow`        | 1.0.103 | 1.0.104 | patch      |
| `indicatif`     | 0.18.5  | 0.18.6  | patch      |

Note: the commit message says "1.13.0 to 1.13.1" but the
`Cargo.lock` actually transitions 1.12.4 → 1.13.1, skipping the
1.13.0 release. This is consistent with Dependabot's `cargo` ecosystem
configuration which can pick up non-contiguous updates.

**No GHSA / CVE was observed for the specific version ranges that
shipped.** The `regex` 1.13.0 / 1.13.1 release notes are additive
(API additions and performance improvements) with no security
advisories referenced in the upstream CHANGELOG. `regex-automata`
follows the same trajectory. `anyhow` 1.0.103 → 1.0.104 is a
behavior-preserving patch. `indicatif` 0.18.5 → 0.18.6 is a
behavior-preserving patch. None of these dependencies ship new
externally-reachable attack surface.

**Recommended action:** none. Continue to accept Dependabot's
`cargo` ecosystem bumps with the existing review guardrails
(dependabot.yml permits only patch + minor, not major; PRs land
through the standard code review and CI gate).

### OBS-7: Required-workflow label-trigger spoofing window closed (this scan window)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Spoofing / Tampering (latent, now mitigated) |
| **CWE** | CWE-863 (Incorrect Authorization), informational only |
| **Files** | `.github/workflows/em-ci-routed-rust.yml` (lines `on.pull_request.types`, `jobs.route.if`, `jobs.rust_small_normalize.if`) |
| **Status** | Net hardening; previous-window latent signal absorbed |

**Description:**
Prior to this scan window, the required CI workflow
(`em-ci-routed-rust.yml`) declared
`on.pull_request.types: [opened, synchronize, reopened, labeled]`
and the `route` / `rust_small_normalize` job `if:` guards relied on
`github.event.action != 'labeled' || contains(fromJSON('["allow-github-hosted", "ci-budget-ack"]'), github.event.label.name)`.
Any user with write access to the repo could in principle apply the
`allow-github-hosted` or `ci-budget-ack` label to a PR's head ref to
trigger the required workflow's `labeled` event, which feeds the
routing decision a label-derived signal. While the actual fallback
authorization was already gated to `force_route == "github"` for
operator-driven reroutes, the latent signal was still visible to the
router logic.

This scan window closes that window:

- `on.pull_request.types` is reduced to `[opened, synchronize, reopened]`.
- The `route` job's `if:` simplifies to
  `github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository`.
  Labels are no longer consulted on the routing path. The PR author
  identity guard (head.repo.full_name == github.repository) remains in
  place and unchanged, so fork PRs continue to be rejected.
- The `rust_small_normalize` job's `if:` mirrors the same simplification.
- `docs/status/SWARM_OPERATION.md` documents the new
  operator-driven refresh path explicitly: human operators use
  `workflow_dispatch` input `force_route=github` when self-hosted
  capacity is unavailable, not a label trigger.

**Risk (before this commit):** minimal in practice — the routing
fallback decision was already gated to `force_route == "github"`, and
the required-workflow result is just a single check consumed by
branch protection, so a label-triggered re-run could not change the
merge gate by itself. It could, however, have caused unnecessary CI
minute consumption by re-running the gate against a fresh label
event. That latent cost is now removed.

**Recommended action:** none. The change is a net security
improvement; no remaining action item is attached to this
observation.

## Threat Model

- **Version:** carried over from 2026-06-29
- **Location:** `.factory/threat-model.md`
- **File last-modified:** 2026-07-05
- **Age at scan time:** 22 days (within the 90-day regen window)
- **STRIDE coverage:** Spoofing, Tampering, Repudiation, Information
  Disclosure, Denial of Service, Elevation of Privilege
- **Trust boundaries enumerated:** 6 (TB-1 through TB-6)
- **Mitigations verified in code:** 10 (table in threat model)
- **Open risks tracked:** 3 (OM-1 Reconcile, OM-2 floating action
  versions, OM-3 output sanitizer OSC edge case)
- **Next regen due:** 2026-09-25 (90 days from generation) or sooner
  on any material change to TB-1 through TB-6

The trust boundaries, mitigations, and open risks from the threat
model remain valid. This scan window's refactor confines itself to
the engine crate's internal executor / scheduler separation and to a
new event variant for the durable upload checkpoint; it does not
touch TB-1 (registry crossing), TB-3 (env -> CLI),
TB-4 (webhook), TB-5 (CI secrets), or TB-6 (cargo metadata JSON).
The scheduler now spawns per-level threads under
`thread::spawn` and joins via `handle.join()` with an explicit
poison-handling fallback (`poisoned_lock` helper in
`execute_package.rs`); both `state` and `event_log` `Arc<Mutex<...>>`
sites map poison to `anyhow::anyhow!("... lock poisoned ...")`
rather than panicking, so TB-2's event/state contract holds under
poison too.

## Scan Metadata

- **Commits scanned:** 1
- **Commit:** `a81869e3689ad3abec37fde76686e6a248d15c2f deps(deps):
  bump regex from 1.13.0 to 1.13.1 (#187)`
- **Commit author:** `dependabot[bot]
  <49699333+dependabot[bot]@users.noreply.github.com>`
- **Commit date:** 2026-07-24 05:47:32 +00:00
- **Scan window:** 2026-07-20 to 2026-07-27 (last 7 days, UTC)
- **Scan duration:** ~3 minutes
- **Branch:** `droid/security-report-2026-07-27`
- **Severity threshold:** medium
- **Skills used:** threat-model check (carry-over, in-window),
  commit-security-scan, vulnerability-validation (against the
  auth/encryption/HTTP/webhook/sanitizer surface), security-review
  (no patches required; no findings at MEDIUM or above)
- **Build status:** `cargo check --workspace --all-targets`
  succeeds. No clippy regressions introduced by the executor /
  scheduler split (lint exceptions file
  `policy/clippy-exceptions.toml` updated path-only from
  `engine/publish.rs:142` to `engine/execute_package.rs:94`).
- **Test status:**
  - `cargo test -p shipper-core --lib state::rebuild`:
    `13 passed; 0 failed; 0 ignored` (covers the new
    `EventType::PackageUploaded` rebuild arm and the
    `ReadinessStarted` backward-compat arm).
  - `python3 scripts/ci/normalize-routed-rust-result.py --test`:
    `routed result normalization tests passed` (6 inline test
    cases covering direct-fallback, self-hosted success,
    cancelled-self-hosted + successful-fallback, failed-self-hosted,
    cancelled-self-hosted + failed-fallback, and hard-fail
    github routing).
- **Functional delta vs. previous scan reference (`0589541`):**
  40 files / +3,072 insertions / −1,896 deletions. Files touched:
  - `.factory/security/reports/security-report-2026-07-13.md`
    (prior report carry-over, now superseded by this report).
  - `.github/workflows/em-ci-routed-rust.yml` (PR types shrink;
    `route` and `rust_small_normalize` `if:` simplify; inline
    `python3 <<'PY' ... PY` block replaced by
    `python3 scripts/ci/normalize-routed-rust-result.py`; new
    `needs: [route, rust_small_cx43, rust_small_cpx42,
    rust_small_cx53]` chain on `rust_small_github` so the
    GitHub-hosted fallback re-runs only when the cancelled
    self-hosted lane cannot complete; `timeout-minutes` raised
    45 → 75 for the GitHub-hosted lane).
  - `.github/workflows/runner-routing-guard.yml` (added actionlint
    install with SHA-256 pin; added `actionlint -shellcheck=""`
    workflow validation step; added
    `python3 scripts/ci/normalize-routed-rust-result.py --test`
    self-test step).
  - `CHANGELOG.md` (one bullet under `### Changed`: "Package
    execution timeout policy").
  - `Cargo.lock` (regex 1.12.4 → 1.13.1, regex-automata
    0.4.14 → 0.4.16, anyhow 1.0.103 → 1.0.104, indicatif
    0.18.5 → 0.18.6).
  - `crates/shipper-cli/src/lib.rs` (handles the new
    `EventType::PackageUploaded` variant in `event_type_name`
    and `summarize_event`).
  - `crates/shipper-cli/tests/bdd_parallel.rs` (one test fixture
    additive: `--readiness-timeout 250ms --readiness-poll 10ms`
    to make the failure-stops-subsequent-levels fixture
    deterministic, and a 30s → 5s `recv_timeout` shortening on
    the registry mock).
  - `crates/shipper-cli/tests/bdd_resume.rs` (new
    `SHIPPER_FAKE_PUBLISH_STDOUT` / `SHIPPER_FAKE_PUBLISH_STDERR`
    env vars on the fake cargo proxy; new positive test
    `fake_cargo_proxy_treats_shell_metacharacters_as_output` that
    pins the fake cargo's quoting behavior; new
    `TestRegistry::join` request-count assertion with `stop`
    flag for deterministic teardown).
  - `crates/shipper-cli/tests/e2e_expanded.rs` (Windows-path
    normalization in `normalize_tempdir_paths` and
    `normalize_stderr`; no behavioral change).
  - `crates/shipper-core/src/engine/AGENTS.md` and
    `crates/shipper-core/src/engine/CLAUDE.md`
    (documentation-only).
  - `crates/shipper-core/src/engine/mod.rs` (massive deletions:
    removed the embedded sequential publish loop and the
    `record_terminal_resume_skip` / `apply_resume_from_gate` /
    `verify_published_after_started` re-exports; added
    `pub(crate) mod execute_package;` and re-exports
    `preflight::PreflightRunOptions`).
  - `crates/shipper-core/src/engine/execute_package.rs` (NEW —
    1804 lines: `PackagePublishResult`, `run_sequential_scheduler`,
    `synchronize_sequential_state`, `poisoned_lock`,
    `commit_transition`, `commit_attempt_transition`,
    `commit_with_attempt_detail_transition`,
    `commit_pending_with_attempt_detail_transition`,
    `commit_attempt_detail_pending`,
    `emit_retry_backoff`, `record_retry_backoff`,
    `record_rate_limit_observed`, `flush_event_log`,
    `record_readiness_event`, `wait_after_retry`,
    `write_reconciliation_report_best_effort`,
    `sequential_cargo_timeout`, and `publish_package` /
    `publish_package_with_timeout`).
  - `crates/shipper-core/src/engine/parallel/AGENTS.md` and
    `crates/shipper-core/src/engine/parallel/CLAUDE.md`
    (documentation-only).
  - `crates/shipper-core/src/engine/parallel/mod.rs` (visibility
    changes: `pub(super)` → `pub(crate)` for `SendReporter`,
    `policy`, `readiness`, `reconcile`, `webhook`; new
    `pub(crate) mod scheduler;`).
  - `crates/shipper-core/src/engine/parallel/policy.rs`
    (one-line import reference update).
  - `crates/shipper-core/src/engine/parallel/readiness.rs`
    (uses `crate::registry::RegistryClient` instead of
    `shipper_registry::HttpRegistryClient as RegistryClient`;
    local-path readiness now returns `Ok(bool)` directly via
    `shipper_sparse_index::contains_version`).
  - `crates/shipper-core/src/engine/parallel/reconcile.rs`
    (visibility change `pub(super)` → `pub(crate)` for
    `reconcile_ambiguous_upload`; import symbol swap).
  - `crates/shipper-core/src/engine/parallel/scheduler.rs`
    (NEW — 101 lines: `run_publish_level` consuming
    `execute_package::publish_package`).
  - `crates/shipper-core/src/engine/parallel/tests.rs`
    (~1,039-line net add: `error_class_rank`,
    `sort_attempt_history_by_fields`, `test_registry_client`,
    trait impl shim for `crate::engine::Reporter`, and new
    property / proptest coverage for the new executor paths).
  - `crates/shipper-core/src/engine/publish/ambiguous.rs`
    (deleted; content moved into `execute_package.rs`).
  - `crates/shipper-core/src/engine/publish/mod.rs` (one-line
    surface cleanup).
  - `crates/shipper-core/src/engine/readiness.rs` (deleted;
    content moved into `execute_package.rs`).
  - `crates/shipper-core/src/engine/retry.rs` (deleted; content
    moved into `execute_package.rs`).
  - `crates/shipper-core/src/state/events/proptests.rs` (one
    line added: `Just(EventType::PackageUploaded)` so the new
    variant reaches the proptest corpus).
  - `crates/shipper-core/src/state/rebuild.rs` (new event arm
    `EventType::PackageUploaded => Uploaded`; new tests
    `rebuild_package_uploaded_projects_uploaded_until_published`,
    `rebuild_interrupt_resumes_from_uploaded_checkpoint`,
    `rebuild_rejects_corrupt_event_log`, and
    `rebuild_readiness_started_still_projects_uploaded_for_compatibility`).
  - `crates/shipper-types/src/lib.rs` (new
    `EventType::PackageUploaded` variant; proptest corpus count
    raised `0u8..22` → `0u8..23` in
    `event_type_all_variants_roundtrip` and
    `event_type_debug_never_panics` so the new variant is
    covered).
  - `docs/INVARIANTS.md` (Updated `Uploaded recovery checkpoint`
    section to refer to `EventType::PackageUploaded` as the
    primary durable checkpoint, with `ReadinessStarted` retained
    as a backward-compatibility bridge).
  - `docs/NO_PANIC_POLICY.md` (Path + snippet updated from
    `engine/parallel/publish.rs` to `engine/execute_package.rs`).
  - `docs/POLICY_ALLOWLISTS.md` (Path-only update; same as
    NO_PANIC_POLICY.md).
  - `docs/ci/ripr.md` (path reference update).
  - `docs/decrating-plan.md` (path-only update).
  - `docs/status/SWARM_OPERATION.md` (one new paragraph
    documenting the `workflow_dispatch` `force_route=github`
    operator refresh path).
  - `docs/structure.md` (Updated the `crates/shipper-core/src/`
    ASCII tree to show the new `engine/execute_package.rs` and
    the renamed `engine/parallel/{mod,scheduler,readiness}.rs`
    files; old `engine/parallel/publish.rs` line removed).
  - `policy/clippy-exceptions.toml` (path reference update
    `engine/publish.rs:142` → `engine/execute_package.rs:94`).
  - `policy/non-rust-allowlist.toml` (one new entry for
    `scripts/ci/normalize-routed-rust-result.py`: kind
    `ci_policy_script`, surface `ci`, classification `script`,
    owner `release/ci`, reason "Normalize routed Rust workflow
    results into one deterministic blocking signal.",
    `covered_by = [".github/workflows/em-ci-routed-rust.yml",
    ".github/workflows/runner-routing-guard.yml"]`,
    `created = "2026-07-18"`, `review_after = "2026-10-18"`).
  - `policy/process-allowlist.toml` (process allowlist expanded
    by three tools used in `runner-routing-guard.yml`:
    `curl`, `sha256sum`, `tar`; matching reason field updated
    in-line).
  - `policy/ripr-suppressions.toml` (path reference update).
  - `scripts/ci/normalize-routed-rust-result.py` (NEW — 165
    lines, the normalized-routing helper called from
    `em-ci-routed-rust.yml::rust_small_normalize` and
    self-tested from `runner-routing-guard.yml`).

## Lenses Applied

Per the shipper review-invariants context:

1. **STRIDE** for the entire security-sensitive surface (see threat
   model). All six categories checked against the auth, encryption,
   registry HTTP, webhook transport, state persistence, and
   subprocess invocation surfaces.
2. **OWASP Top 10** for any web/CLI surface: A01 (Broken Access
   Control) verified by the advisory file lock + plan-ID validation;
   A02 (Cryptographic Failures) verified by `shipper-encrypt`
   (AES-256-GCM + PBKDF2 100k iterations + per-call random
   salt/nonce) and the `pbkdf2/hmac/sha2/aes-gcm` ignore rules in
   `dependabot.yml`; A03 (Injection) verified by the absence of
   `sh -c`/shell pipelines in subprocess invocations and by
   `taiki-e`/`std::process::Command` controlled-argument paths,
   plus the new `fake_cargo_proxy_treats_shell_metacharacters_as_output`
   test that pins the fake cargo proxy's behavior under
   shell-metacharacter STDOUT; A04 (Insecure Design) verified by
   the events-as-truth invariant and the schema-versioned
   `state.json` projection; A05 (Security Misconfiguration)
   verified by per-job `permissions:` blocks in every workflow;
   A06 (Vulnerable & Outdated Components) is this scan's primary
   lens (`Cargo.lock` delta is the only externally-reachable
   surface change); A07 (Identification and Authentication
   Failures) verified by the cargo-conventional token precedence
   and the OIDC dual-env-var detection; A08 (Software and Data
   Integrity Failures) verified by the receipt/events/state
   triplet; A09 (Security Logging and Monitoring Failures)
   verified by `events.jsonl` authoritative recording; A10 (SSRF)
   verified by `https://crates.io` as the only first-party base
   URL and the `reqwest` default client builder with timeout.
3. **OWASP LLM Top 10** is not directly applicable (no LLM
   boundary inside the engine). The Droid workflow layer is
   reviewed separately by `droid-review` and `droid-security-scan`
   automations.
4. **STRIDE spot-checks at the auth boundary**: `AuthInfo` carries
   `source: TokenSource` so diagnostic output never accidentally
   displays the token; `mask_token` is exercised by proptest with
   `[A-Za-z0-9]` ASCII + edge lengths (0, 1, 8, 9, 200, 500) and
   confirmed never to expose the middle of tokens longer than 8
   characters. This scan window does not touch the auth resolver.
5. **STRIDE spot-checks at the transition boundary
   (`transition.rs` and the new `execute_package.rs::commit_*_transition`
   shims)**: `validate_attempt_detail` rejects `package@version`
   mismatches before the event log write, preventing an attempt
   detail intended for one package from being recorded against
   another. The clone-then-mutate pattern in
   `transition::persist` keeps the caller's in-memory state
   unchanged when the event cannot be appended, and surfaces a
   single error when either the event-log write or the
   state-projection write fails, preserving the events-as-truth
   contract. The new `EventType::PackageUploaded` is a no-data
   enum variant (no `String` payload), so a malformed
   `events.jsonl` line for this variant would be rejected by
   serde and the rebuild contract holds without any new
   validation code.
6. **STRIDE spot-checks at the subprocess boundary
   (`ops::cargo::cargo_publish` and the new
   `execute_package::publish_package_with_timeout`)**:
   `cargo_publish` builds its `args: Vec<&str>` from
   `["publish", "-p", package_name, "--registry", registry_name,
   ...]` where `package_name` and `registry_name` are
   `&str` slices read from the `PlannedPackage` (built from
   cargo's own `cargo_metadata::Package` whose `name` is
   constrained to `^[a-zA-Z0-9_-]+$` by cargo's manifest parser)
   and from the explicit `Registry::name` configured in
   `[registry]` / `--registry`. No shell interpolation occurs,
   so a hostile or malformed package name or registry name
   cannot break out of argv and into a shell parser.

## References

- [CWE Database](https://cwe.mitre.org/)
- [STRIDE Threat Model](https://learn.microsoft.com/en-us/security/engineering/threat-modeling)
- [OWASP Top 10](https://owasp.org/Top10/)
- [docs/INVARIANTS.md](../../../../INVARIANTS.md) - events-as-truth contract
- [docs/status/SWARM_OPERATION.md](../../../../status/SWARM_OPERATION.md) -
  active-development / release-authority split
- [SECURITY.md](../../../../SECURITY.md) - project security policy
- `.factory/threat-model.md` - carried-over threat model (2026-06-29)
- Previous weekly scan:
  `.factory/security/reports/security-report-2026-07-13.md`
- Prior reference:
  `.factory/security/reports/security-report-2026-06-29.md`
