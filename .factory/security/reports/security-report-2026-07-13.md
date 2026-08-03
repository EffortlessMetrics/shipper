# Security Scan Report

**Generated:** 2026-07-13
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

The weekly scan of `droid/security-report-2026-07-13` over the last 7
days (2026-07-06 through 2026-07-13, UTC) examined the single commit
that landed on `main` in the window: `0589541 fix(engine): persist
attempt details through transition boundary`. That commit is the
wholesale migration commit into the `shipper-swarm` working repository
and carries 1,319 changed files / ~185k insertions. The functional delta
inside the engine crate is the new `commit_with_attempt_detail`,
`commit_pending_with_attempt_detail`, and
`commit_attempt_detail_pending` helpers in
`crates/shipper-core/src/engine/transition.rs` plus the matching call
sites in the sequential and parallel publish loops. These helpers
persist attempt details through the same event-first transition
boundary as the corresponding state projection, so the
events-as-truth invariant is now stronger, not weaker.

No application code in an auth, token-resolution, encryption,
state-persistence, or subprocess-invocation path was altered in a way
that introduces a finding at the configured `medium` severity
threshold.

The repository remains in a strong security posture: `unsafe_code =
"forbid"` is enforced workspace-wide, every event/state/receipt
triplet is preserved, the output sanitizer redacts tokens before
persistence, the encryption crate uses AES-256-GCM with PBKDF2 (100,000
iterations) and per-call random salt + nonce, the registry HTTP client
enforces timeouts, and the fuzzer corpus (`fuzz/fuzz_targets/`) covers
token resolution, encryption, output redaction, plan building, retry,
and webhook payload construction.

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
are recorded for the next weekly scan and for engineering awareness; no
remediation is required for this report.

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
tags rather than commit SHAs. Examples observed across the scan window:
`actions/checkout@v7.0.0`, `dtolnay/rust-toolchain@stable`,
`dtolnay/rust-toolchain@nightly`, `dtolnay/rust-toolchain@v1`,
`taiki-e/install-action@v2`, `taiki-e/install-action@cargo-audit`,
`actions/cache@v6`, `actions/upload-artifact@v7`,
`actions/download-artifact@v8`, `codecov/codecov-action@v7`,
`softprops/action-gh-release@v3`,
`rust-lang/crates-io-auth-action@v1`. The Droid-related actions
(`EffortlessMetrics/droid-action-safe@7c1377c...`,
`oven-sh/setup-bun@0c5077e5...`) and the direct `actions/checkout`
reference in the Droid workflows are SHA-pinned.

**Risk:** A compromise of an upstream major tag could push arbitrary
code into CI. Most jobs hold `contents: read` and run on self-hosted
runners with fork-PR guards. The release workflow (`release.yml`) uses
`rust-lang/crates-io-auth-action@v1` to exchange an OIDC token for a
short-lived crates.io token, but that path is gated by
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

**Recommended Hardening (Optional):**
Pin all third-party Actions to commit SHAs (mirror the pattern already
used for `EffortlessMetrics/droid-action-safe` and
`oven-sh/setup-bun`). Dependabot's group updates will continue to bump
the SHA in lockstep. Trade-off: reduced upstream agility for tighter
supply-chain posture. Not required for this scan window.

### OBS-2: Single commit in scope is a wholesale migration

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Not applicable (informational) |
| **CWE** | Not applicable |
| **Files** | (commit scope) |
| **Status** | Migration artefact; informational only |

**Description:**
The single commit in scope is the wholesale migration of the
`EffortlessMetrics/shipper` release-authority repository into the
`shipper-swarm` working repository. It carries 1,319 changed files /
~185k insertions. The functional changes introduced by this commit
beyond the migration are concentrated in
`crates/shipper-core/src/engine/` (event-boundary persistence for
sequential and parallel publishing, plus centralized
terminal/parallel/sequential failure-transition routing) and the
supporting test, snapshot, and fuzz-target surface.

**Implication for the scan:** the threat model
(`.factory/threat-model.md`, generated 2026-06-29) applies to the
migrated tree as a whole. The functional delta relative to the prior
source of truth lies entirely within event/state transition internals;
it does not touch the auth resolver, encryption crate, HTTP client,
output sanitizer, webhook transport, state store, or subprocess
invocation. No delta in any of those surfaces is in scope for this
weekly window.

### OBS-3: Whitespace-only token values are treated as valid credentials

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Spoofing / Tampering (low impact) |
| **CWE** | CWE-20 (Improper Input Validation), informational only |
| **File** | `crates/shipper-core/src/ops/auth/resolver.rs:88-97` |
| **Status** | Matches cargo behaviour; no fix recommended |

**Description:**
The `resolve_token` implementation treats an env var whose value is a
whitespace-only string as a valid token and returns it as the resolved
credential (the empty-check is `!token.is_empty()`, not
`!token.trim().is_empty()`). This behaviour is pinned by the
`resolve_token_whitespace_only_env_is_not_skipped` test in the same
file. There is no upstream remediation because cargo itself preserves
whitespace-only tokens through `cargo login` and through the env-var
precedence chain.

**Risk:** minimal. A user who explicitly sets
`CARGO_REGISTRY_TOKEN="   "` in their environment gets a whitespace
token; the only consequence is that any subsequent publish attempt
will be rejected by crates.io with HTTP 403, which the existing
`ErrorClass::Permanent` classifier surfaces to the user. No token
is leaked, no privilege escalation is possible, and no other
caller is reachable from this state.

**Recommended action:** none. Documented for future auditability.

### OBS-4: Output sanitizer OSC without terminator consumes to EOF (carried-over OM-3)

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Denial of Service (bounded) |
| **CWE** | CWE-400 (Uncontrolled Resource Consumption), informational only |
| **File** | `crates/shipper-output-sanitizer/src/lib.rs` (`strip_ansi`) |
| **Status** | Accepted behaviour, tracked under OM-3 in `.factory/threat-model.md` |

**Description:** `strip_ansi` consumes an unterminated OSC sequence
(`\x1b]...`) to EOF, including any trailing newline. The behaviour is
pinned by `osc_without_terminator_consumes_to_eof` and
`osc_without_terminator_does_not_panic_with_following_lines` tests in
the same file. There is no security impact (no panic, no out-of-memory
condition, no leaked token), but evidence truncation could cause
operator confusion during incident review.

**Recommended action:** none. Documented under OM-3.

### OBS-5: `mask_token` exposes first and last four characters

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

### OBS-6: Commit-change attack surface is concentrated in transition internals

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Repudiation (strengthened, not weakened) |
| **CWE** | CWE-778 (Insufficient Logging), informational only |
| **File** | `crates/shipper-core/src/engine/transition.rs` |
| **Status** | Net defensive hardening of events-as-truth invariant |

**Description:**
The new `commit_with_attempt_detail`, `commit_pending_with_attempt_detail`,
and `commit_attempt_detail_pending` functions in
`crates/shipper-core/src/engine/transition.rs` (and their matching
`commit_*_transition` shims in
`crates/shipper-core/src/engine/parallel/publish.rs`) persist attempt
details through the same event-first transition boundary as the
corresponding state projection. The clone-then-mutate pattern in
`transition::persist` ensures that the caller's in-memory state is not
mutated until both the event log write and the state write have
succeeded, and the `validate_attempt_detail` helper rejects attempt
details whose `package@version` key does not match the transition's key
(`transition::validate_attempt_detail`).

These changes strengthen the events-as-truth invariant rather than
weakening it: a successful terminal transition cannot be written
without its matching attempt timeline, and a package mismatch in the
attempt detail aborts the transition. The existing `redact_sensitive`
path through `ops::cargo::tail_lines` -> `cargo_publish::stdout_tail`
/ `stderr_tail` -> `classify_cargo_failure` -> `AttemptDetail::redacted_message`
remains unchanged, so token redaction continues to flow correctly into
persisted attempt detail.

**Recommended action:** none. Track the remaining
`Full AttemptDetail replay from events` follow-up that the commit
message explicitly defers.

## Threat Model

- **Version:** carried over from 2026-06-29
- **Location:** `.factory/threat-model.md`
- **Age at scan time:** 14 days (within the 90-day regen window)
- **STRIDE coverage:** Spoofing, Tampering, Repudiation, Information
  Disclosure, Denial of Service, Elevation of Privilege
- **Trust boundaries enumerated:** 6 (TB-1 through TB-6)
- **Mitigations verified in code:** 10 (table in threat model)
- **Open risks tracked:** 3 (OM-1 Reconcile, OM-2 floating action
  versions, OM-3 output sanitizer OSC edge case)
- **Next regen due:** 2026-09-27 (90 days) or sooner on any change
  to TB-1 through TB-6

The trust boundaries, mitigations, and open risks from the threat
model remain valid. The transition-boundary hardening in this commit
strengthens TB-2 (Workspace on local disk <-> `shipper-core` engine)
and TB-3 (User's environment <-> `shipper` CLI) by ensuring that
`events.jsonl` and `state.json` advance together for any attempt-detail
projection, not just for terminal state transitions.

## Scan Metadata

- **Commits scanned:** 1
- **Commit:** `0589541 fix(engine): persist attempt details through
  transition boundary`
- **Commit author:** Steven Zimmerman, CPA
  <15812269+EffortlessSteven@users.noreply.github.com>
- **Commit date:** 2026-07-13 01:26:18 -0400
- **Scan window:** 2026-07-06 to 2026-07-13 (last 7 days, UTC)
- **Scan duration:** < 2 minutes
- **Branch:** `droid/security-report-2026-07-13`
- **Severity threshold:** medium
- **Skills used:** threat-model check (carry-over),
  commit-security-scan, vulnerability-validation (against the
  auth/encryption/HTTP/webhook/sanitizer surface), security-review
- **Build status:** `cargo build` succeeds, `cargo clippy
  --workspace --all-targets -- -D warnings` succeeds.
- **Test status:** `cargo test -p shipper-core --lib transition::` -
  10 passed, 0 failed (the new transition functions are covered).
- **Files inspected (security-sensitive surface):**
  - `crates/shipper-core/src/engine/transition.rs` (new)
  - `crates/shipper-core/src/engine/parallel/publish.rs`
  - `crates/shipper-core/src/engine/retry.rs`
  - `crates/shipper-core/src/engine/publish/finalize.rs`
  - `crates/shipper-core/src/engine/mod.rs`
  - `crates/shipper-core/src/state/rebuild.rs`
  - `crates/shipper-core/src/state/events/mod.rs`
  - `crates/shipper-core/src/state/execution_state/mod.rs`
  - `crates/shipper-core/src/state/store/fs.rs`
  - `crates/shipper-core/src/ops/auth/resolver.rs`
  - `crates/shipper-core/src/ops/auth/credentials.rs`
  - `crates/shipper-core/src/ops/auth/oidc.rs`
  - `crates/shipper-core/src/ops/auth/mod.rs`
  - `crates/shipper-core/src/ops/cargo/mod.rs`
  - `crates/shipper-encrypt/src/lib.rs`
  - `crates/shipper-registry/src/http.rs`
  - `crates/shipper-registry/src/context.rs`
  - `crates/shipper-core/src/engine/parallel/webhook.rs`
  - `crates/shipper-webhook/src/lib.rs`
  - `crates/shipper-output-sanitizer/src/lib.rs`
  - `fuzz/fuzz_targets/auth_token_resolve.rs`
  - `fuzz/fuzz_targets/redact_output.rs`
  - `crates/shipper-cli/src/doctor/redaction.rs`
  - `crates/shipper-cli/src/doctor/checks/auth.rs`
  - `crates/shipper-cli/src/doctor/checks/encryption.rs`
  - `.github/workflows/droid.yml`
  - `.github/workflows/droid-review.yml`
  - `.github/workflows/droid-security-scan.yml`
  - `.github/workflows/release.yml`
  - `.github/workflows/ci.yml`
  - `.github/workflows/coverage.yml`
  - `.github/workflows/em-ci-routed-rust.yml`
  - `.github/workflows/mutation.yml`
  - `.github/workflows/fuzz.yml`
  - `.github/workflows/ripr.yml`
  - `.github/workflows/live-runner-interruption-rehearsal.yml`
  - `.github/workflows/runner-routing-guard.yml`
  - `.github/workflows/architecture-guard.yml`
  - `.github/dependabot.yml`
  - `.github/settings.yml`
  - `SECURITY.md`

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
   `taiki-e`/`std::process::Command` controlled-argument paths; A04
   (Insecure Design) verified by the events-as-truth invariant and
   the schema-versioned `state.json` projection; A05 (Security
   Misconfiguration) verified by per-job `permissions:` blocks in
   every workflow; A06 (Vulnerable & Outdated Components) is this
   scan's primary lens; A07 (Identification and Authentication
   Failures) verified by the cargo-conventional token precedence and
   the OIDC dual-env-var detection; A08 (Software and Data
   Integrity Failures) verified by the receipt/events/state
   triplet; A09 (Security Logging and Monitoring Failures) verified
   by `events.jsonl` authoritative recording; A10 (SSRF) verified
   by `https://crates.io` as the only first-party base URL and the
   `reqwest` default client builder with timeout.
3. **OWASP LLM Top 10** is not directly applicable (no LLM boundary
   inside the engine). The Droid workflow layer is reviewed
   separately by `droid-review` and `droid-security-scan`
   automations.
4. **STRIDE spot-checks** at the auth boundary: `AuthInfo` carries
   `source: TokenSource` so diagnostic output never accidentally
   displays the token; `mask_token` is exercised by proptest with
   `[A-Za-z0-9]` ASCII + edge lengths (0, 1, 8, 9, 200, 500) and
   confirmed never to expose the middle of tokens longer than 8
   characters.
5. **STRIDE spot-checks** at the transition boundary
   (`transition.rs`): `validate_attempt_detail` rejects
   `package@version` mismatches before the event log write,
   preventing an attempt detail intended for one package from being
   recorded against another. The clone-then-mutate pattern in
   `transition::persist` keeps the caller's in-memory state
   unchanged when the event cannot be appended, and surfaces a
   single error when either the event-log write or the
   state-projection write fails, preserving the events-as-truth
   contract.

## References

- [CWE Database](https://cwe.mitre.org/)
- [STRIDE Threat Model](https://learn.microsoft.com/en-us/security/engineering/threat-modeling)
- [OWASP Top 10](https://owasp.org/Top10/)
- [docs/INVARIANTS.md](../../../INVARIANTS.md) - events-as-truth contract
- [docs/status/SWARM_OPERATION.md](../../../status/SWARM_OPERATION.md) -
  active-development / release-authority split
- [SECURITY.md](../../../../SECURITY.md) - project security policy
- `.factory/threat-model.md` - carried-over threat model (2026-06-29)
- Previous weekly scan: `.factory/security/reports/security-report-2026-07-06.md`
