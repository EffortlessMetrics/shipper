# Security Scan Report

**Generated:** 2026-06-01
**Scan Type:** Weekly Scheduled
**Repository:** EffortlessMetrics/shipper
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

## Scan Results

No security vulnerabilities at or above medium severity were identified in this scan.

### Areas Scanned

The following security-critical areas were analyzed:

1. **Token/Credential Handling**
   - Token resolution from environment variables (`CARGO_REGISTRY_TOKEN`, `CARGO_REGISTRIES_*_TOKEN`)
   - Credentials file handling (`$CARGO_HOME/credentials.toml`)
   - OIDC fallback authentication
   - Token masking via `mask_token()`

2. **State Persistence**
   - State file writes (`.shipper/state.json`, `events.jsonl`, `receipt.json`)
   - Atomic write operations (temp + rename with fsync)
   - Lock file mechanisms

3. **Command Execution**
   - Subprocess spawning via `std::process::Command`
   - Argument handling (no shell evaluation)
   - `cargo publish` execution with registry flag

4. **Network Operations**
   - Registry API interactions
   - Retry/backoff logic with exponential backoff
   - Rate limit handling (HTTP 429)
   - Webhook HMAC-SHA256 signatures

5. **Encryption**
   - AES-256-GCM implementation
   - PBKDF2 key derivation (100,000 iterations)
   - Webhook secret handling with `skip_serializing_if`

6. **Configuration**
   - TOML config file parsing
   - CLI argument overrides
   - Environment variable handling

## Appendix

### Threat Model
- Version: Newly generated (2026-06-01)
- Location: .factory/threat-model.md
- Status: Generated as part of this scan

### Scan Metadata
- Commits Scanned: 1 (merge commit from shipper-swarm)
- Files Analyzed: crates/shipper-core/src/, crates/shipper-cli/src/
- Scan Duration: ~5 minutes
- Skills Used: threat-model-generation, commit-security-scan, security-review

### Security Controls Verified

| Control | Status | Notes |
|---------|--------|-------|
| Token masking | ✅ Verified | `mask_token()` shows first 4 + last 4 chars |
| Atomic writes | ✅ Verified | temp + rename with fsync |
| No shell evaluation | ✅ Verified | Uses `std::process::Command` with args array |
| HMAC webhooks | ✅ Verified | SHA-256 signature on `X-Hub-Signature-256` |
| AES-256-GCM | ✅ Verified | 100k PBKDF2 iterations |
| `unsafe_code = forbid` | ✅ Verified | Workspace-wide enforced |
| Lock timeout | ✅ Verified | Stale lock detection |

### Recommendations

The codebase demonstrates good security posture. No immediate remediation is required.

Maintain current security practices:
- Continue enforcing `unsafe_code = "forbid"` workspace-wide
- Keep token masking in all logging paths
- Maintain atomic write patterns for state files
- Continue using parameterized subprocess arguments

### References
- [CWE Database](https://cwe.mitre.org/)
- [STRIDE Threat Model](https://docs.microsoft.com/en-us/azure/security/develop/threat-modeling-tool-threats)
