# Security Scan Report

**Generated:** 2026-05-25
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

No security vulnerabilities at or above medium severity were identified in the files changed in the last 7 days.

### Areas Reviewed

The security scan focused on the following high-priority surfaces:

1. **Authentication & Token Handling** (`ops/auth/`)
   - Token resolution from environment variables and credentials files
   - OIDC trusted publishing detection
   - Token redaction in output

2. **Process Execution** (`ops/process/`, `ops/cargo/`)
   - Command injection prevention
   - Output sanitization
   - Timeout handling

3. **Lock Mechanism** (`ops/lock/`)
   - File-based advisory locking
   - Stale lock detection
   - Race condition analysis

4. **Configuration** (`shipper-config/`)
   - Secret handling in webhooks
   - Encryption passphrase handling

### Security Controls Verified

The codebase implements the following security controls:

- ✅ `unsafe_code = "forbid"` enforced workspace-wide
- ✅ Tokens are opaque strings and never logged
- ✅ All cargo output redacted via `shipper_output_sanitizer`
- ✅ Whitespace-trimmed empty tokens treated as absent
- ✅ OIDC detection requires both env vars (`ACTIONS_ID_TOKEN_REQUEST_URL` and `ACTIONS_ID_TOKEN_REQUEST_TOKEN`)
- ✅ Events are append-only (events-as-truth invariant)

### Known Documented Limitations

The following limitation is documented and considered acceptable:

1. **Lock Race Condition (TOCTOU)**: `LockFile::acquire` uses check-then-create pattern. Under tight concurrent contention, more than one process may acquire the lock. This is documented in `ops/lock/CLAUDE.md` and is acceptable for the advisory lock use case.

---

## Appendix

### Threat Model
- Version: 2026-05-25 (newly generated)
- Location: .factory/threat-model.md

### Scan Metadata
- Commits Scanned: 1 (522f3f52612f369a64ffdbf009930f0c640fd4e8)
- Scan Duration: ~5 minutes
- Files Reviewed: 50+ Rust source files
- Skills Used: threat-model-generation, manual code review

### References
- [CWE Database](https://cwe.mitre.org/)
- [STRIDE Threat Model](https://docs.microsoft.com/en-us/azure/security/develop/threat-modeling-tool-threats)
- [shipper-output-sanitizer crate](https://crates.io/crates/shipper-output-sanitizer)
