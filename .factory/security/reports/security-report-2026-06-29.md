# Security Scan Report

**Generated:** 2026-06-29
**Scan Type:** Weekly Scheduled
**Repository:** EffortlessMetrics/shipper-swarm
**Severity Threshold:** medium

## Executive Summary

| Severity | Count | Auto-fixed | Manual Required |
|----------|-------|------------|-----------------|
| CRITICAL | 0 | 0 | 0 |
| HIGH | 0 | 0 | 0 |
| MEDIUM | 0 | 0 | 0 |
| LOW | 0 | 0 | 0 |

**Total Findings:** 0
**Auto-fixed:** 0
**Manual Review Required:** 0

The weekly scan of `droid/security-report-2026-06-29` over the last 7 days
(2026-06-22 through 2026-06-29) examined the only commit that landed in the
window: `3dc6bd2 ci(deps): bump codecov/codecov-action from 6 to 7 (#127)`.
The commit is the initial migration commit into the `shipper-swarm` working
repository and surfaces a single functional change: a new
`.github/workflows/coverage.yml` file that uses `codecov/codecov-action@v7`
in place of the prior v6 invocation pattern. No application code, no
auth/token-resolution path, no encryption path, no state-persistence path,
and no subprocess invocation surface was altered in a way that introduces a
security finding at the configured severity threshold.

## Critical Findings

None.

## High Findings

None.

## Medium Findings

None.

## Low Findings

None.

## Observations (Below Severity Threshold, Not Reported as Findings)

These are not findings under the configured `medium` threshold. They are
recorded for the next weekly scan and for engineering awareness; no
remediation is required for this report.

### OBS-1: Coverage workflow uses a floating tag for codecov-action

| Attribute | Value |
|-----------|-------|
| **Severity** | LOW (informational) |
| **STRIDE Category** | Tampering (supply chain) |
| **CWE** | CWE-1357 (Reliance on Untrusted Component) |
| **File** | `.github/workflows/coverage.yml:121, 130` |
| **Status** | Accepted risk, tracked under OM-2 in threat model |

**Description:**
The coverage workflow references the third-party GitHub Action
`codecov/codecov-action@v7` as a floating tag rather than a commit SHA.
The same pattern is used for `taiki-e/install-action@v2`,
`actions/cache@v5`, `actions/upload-artifact@v7`, and
`actions/checkout@actions/checkout@v6.0.2`. The Droid-related
actions (`EffortlessMetrics/droid-action-safe@<sha>`, `oven-sh/setup-bun@<sha>`)
are SHA-pinned.

**Evidence:**
```yaml
- name: Upload coverage to Codecov (main)
  if: ${{ always() && github.event_name == 'push' && hashFiles('lcov.info') != '' && steps.codecov-token.outputs.present == 'true' }}
  uses: codecov/codecov-action@v7
  with:
    token: ${{ secrets.CODECOV_TOKEN }}
    files: lcov.info
    flags: rust-core
    name: shipper-rust-core
    fail_ci_if_error: true
```

**Risk:** A compromise of the upstream tag could push arbitrary code into
CI, gated by the `CODECOV_TOKEN` secret. The job only has
`contents: read` and runs on self-hosted runners.

**Mitigation already in place:**
- Dependabot is configured (`dependabot.yml` -> `github-actions` ecosystem)
  to bump Actions weekly with PRs against this repo.
- The job holds only `contents: read`; it cannot push branches, open PRs,
  or publish releases.
- The `CODECOV_TOKEN` is scoped to coverage upload only; it does not
  provide repo write, OIDC, or release authority.
- SHA-pinning is used for the highest-trust actions (Droid executor,
  Bun installer, repository checkout).

**Recommended Hardening (Optional):**
Pin `codecov-action` to a specific commit SHA and follow Dependabot's
"version updates" pattern with `dependabot.yml` -> `groups: actions: *
update-types: [minor, patch]`. The trade-off is reduced upstream
agility against marginal supply-chain tightening. Not required for
this scan window.

### OBS-2: Initial commit is a wholesale migration

**Description:**
The single commit in scope is the initial commit of the `shipper-swarm`
working repository. It carries 1,313 changed files / 183,672 insertions,
which is the migrated contents of the original `EffortlessMetrics/shipper`
release-authority repository. The functional change introduced by this
commit, beyond the new `coverage.yml` workflow, is a re-homing of the
existing code into the dev repository (per `docs/status/SWARM_OPERATION.md`).

**Implication for the scan:**
The threat model is applied to the migrated tree as a whole (see
`docs/INVARIANTS.md` and the threat model at `.factory/threat-model.md`).
The only delta relative to the prior source of truth is the new
coverage workflow. No delta in the engine (`shipper-core`), CLI
(`shipper-cli`), or facade (`shipper`) crates is in scope for this
weekly window.

## Threat Model

- **Version:** newly generated
- **Location:** `.factory/threat-model.md`
- **STRIDE coverage:** Spoofing, Tampering, Repudiation, Information
  Disclosure, Denial of Service, Elevation of Privilege
- **Trust boundaries enumerated:** 6 (TB-1 through TB-6)
- **Mitigations verified in code:** 10 (table in threat model)
- **Open risks tracked:** 3 (OM-1 Reconcile, OM-2 floating action
  versions, OM-3 output sanitizer OSC edge case)
- **Next regen due:** 2026-09-27 (90 days) or sooner on any change
  to TB-1 through TB-6.

## Scan Metadata

- **Commits scanned:** 1
- **Commit:** `3dc6bd2 ci(deps): bump codecov/codecov-action from 6 to 7 (#127)`
- **Scan window:** 2026-06-22 to 2026-06-29 (last 7 days, UTC)
- **Scan duration:** < 1 minute
- **Branch:** `droid/security-report-2026-06-29`
- **Severity threshold:** medium
- **Skills used:** threat-model-generation, commit-security-scan,
  vulnerability-validation, security-review
- **Files inspected (security-sensitive surface):**
  - `.github/workflows/droid.yml`
  - `.github/workflows/droid-review.yml`
  - `.github/workflows/droid-security-scan.yml`
  - `.github/workflows/release.yml`
  - `.github/workflows/ci.yml`
  - `.github/workflows/coverage.yml`
  - `.github/dependabot.yml`
  - `.github/settings.yml`
  - `crates/shipper-core/src/ops/auth/resolver.rs`
  - `crates/shipper-core/src/ops/auth/credentials.rs`
  - `crates/shipper-core/src/webhook.rs`
  - `crates/shipper-core/src/encryption.rs`
  - `crates/shipper-core/src/state/store/fs.rs`
  - `crates/shipper-registry/src/http.rs`
  - `crates/shipper-registry/src/context.rs`
  - `crates/shipper-output-sanitizer/src/lib.rs`
  - `fuzz/fuzz_targets/auth_token_resolve.rs`
  - `SECURITY.md`

## Lenses Applied

Per the shipper review-invariants context:

1. **STRIDE** for the entire security-sensitive surface (see threat model).
2. **OWASP Top 10** for any web/CLI surface: A01 (Broken Access Control)
   verified by the advisory lock + plan ID validation; A02
   (Cryptographic Failures) verified by `shipper-encrypt` re-export
   and `pbkdf2/hmac/sha2/aes-gcm` ignore rules in `dependabot.yml`;
   A03 (Injection) verified by the lack of `sh -c`/shell pipelines in
   subprocess invocations; A04 (Insecure Design) verified by the
   events-as-truth invariant; A05 (Security Misconfiguration)
   verified by per-job `permissions:` blocks; A06 (Vulnerable &
   Outdated Components) - the scan's primary lens; A07 (Identification
   and Authentication Failures) verified by token-source precedence and
   redaction; A08 (Software and Data Integrity Failures) verified by
   the receipt/events/state triplet; A09 (Security Logging and
   Monitoring Failures) verified by `events.jsonl` authoritative
   recording; A10 (SSRF) - the engine has no user-controlled URL
   construction outside the cargo crates.io domain and the user-
   configured registry base URL.
3. **OWASP LLM Top 10** is not directly applicable (no LLM boundary in
   the engine), but the Droid workflow layer is reviewed separately
   by `droid-review` and `droid-security-scan` automations.

## References

- [CWE Database](https://cwe.mitre.org/)
- [STRIDE Threat Model](https://learn.microsoft.com/en-us/security/engineering/threat-modeling)
- [OWASP Top 10](https://owasp.org/Top10/)
- [docs/INVARIANTS.md](../../INVARIANTS.md) - events-as-truth contract
- [docs/status/SWARM_OPERATION.md](../../status/SWARM_OPERATION.md) -
  active-development / release-authority split
- [SECURITY.md](../../../SECURITY.md) - project security policy
- `.factory/threat-model.md` - newly generated threat model
