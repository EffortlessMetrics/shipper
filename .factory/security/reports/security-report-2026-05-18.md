# Security Scan Report

**Generated:** 2026-05-18
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

## Threat Model

### Overview
Shipper is a Cargo workspace publishing tool that orchestrates the publish pipeline: **plan → preflight → publish → resume**.

### STRIDE Analysis

| Category | Threats | Mitigations in Place |
|---------|---------|---------------------|
| **Spoofing** | Token theft, credential impersonation | Token resolution follows Cargo conventions; tokens never logged; OIDC trusted publishing supported |
| **Tampering** | Malicious cargo publish commands, state file manipulation | Command execution uses safe `Command::new().args()` pattern (no shell expansion); state files written atomically |
| **Repudiation** | Audit gaps in publish evidence | Events-as-truth invariant: `events.jsonl` is append-only authoritative log; `receipt.json` provides audit summary |
| **Information Disclosure** | Token leakage in logs, secrets in output | `redact_sensitive()` sanitizes all output; `mask_token()` shows only first/last 4 chars; tokens never logged |
| **Denial of Service** | Resource exhaustion during publish, infinite retry loops | Exponential backoff with jitter; max_attempts limit; timeout on cargo commands |
| **Elevation of Privilege** | Unsafe resume from corrupted state | Plan ID validation on resume; force_resume requires explicit opt-in |

### Key Security Controls

1. **Token Handling** (`ops/auth/`)
   - Resolution order: `CARGO_REGISTRY_TOKEN` → `CARGO_REGISTRIES_<NAME>_TOKEN` → `credentials.toml`
   - Whitespace-trimmed; empty tokens treated as absent
   - `mask_token()` reveals only first/last 4 characters

2. **Command Execution** (`ops/process/`)
   - Uses `std::process::Command::new(program).args(args)` - NO shell expansion
   - Arguments passed as separate items, never concatenated into shell strings
   - Timeout support prevents hanging processes

3. **Output Sanitization** (`ops/cargo/redact.rs`)
   - `redact_sensitive()` removes tokens from all output
   - Patterns: `CARGO_REGISTRY_TOKEN=*`, `Authorization: Bearer *`, `token = "*"`

4. **State Integrity** (`state/`)
   - Schema version validation on load
   - Events append-only; state is projection
   - Lock file prevents concurrent publishes

## Scan Details

### Commits Scanned (Last 7 Days)
- `702c0e0` - Tighten schema version parsing (#324)

### Files Analyzed
- `crates/shipper-core/src/engine/mod.rs` - Publish orchestration
- `crates/shipper-core/src/ops/process/run.rs` - Command execution
- `crates/shipper-core/src/ops/auth/mod.rs` - Token resolution
- `crates/shipper-core/src/ops/auth/resolver.rs` - Auth resolution
- `crates/shipper-core/src/webhook.rs` - Webhook dispatch
- `crates/shipper-types/src/schema.rs` - Schema validation

### Security Controls Verified

| Control | Status | Evidence |
|---------|--------|----------|
| Safe command execution | ✅ Pass | `Command::new(program).args(args)` pattern used throughout |
| Token redaction | ✅ Pass | `redact_sensitive()` covers all token patterns |
| Token masking | ✅ Pass | `mask_token()` shows first/last 4 chars only |
| No shell injection | ✅ Pass | No `sh -c`, no string interpolation in commands |
| Schema validation | ✅ Pass | Tightened in #324 - rejects invalid formats |
| No hardcoded secrets | ✅ Pass | Static analysis found no credentials in source |
| Atomic file writes | ✅ Pass | State saved via temp file + rename |

### Recent Security Improvement: #324

Commit `702c0e0` ("Tighten schema version parsing") improves security by:
- Validating schema version format (`shipper.<type>.v<N>`)
- Rejecting malformed versions that could cause parsing issues
- Adding comprehensive test coverage for adversarial inputs

## Findings

No vulnerabilities were identified at or above the medium severity threshold.

### What Was Examined

1. **Command Injection** - Searched for `format!` with command arguments, shell expansion patterns (`sh -c`), and string concatenation in command execution. Found safe patterns only: `Command::new(program).args(args)`.

2. **Hardcoded Secrets** - Searched for token patterns, API keys, and credentials in source code. All token handling properly externalized to environment variables or files.

3. **Path Traversal** - Examined file path handling in state operations. All paths are constructed safely with `Path` and `PathBuf`.

4. **Input Validation** - Schema parsing validates format before processing; command arguments are passed as separate items.

5. **Information Disclosure** - Token redaction covers all known patterns; `mask_token()` prevents accidental exposure.

## Conclusion

The codebase demonstrates strong security practices:
- Safe command execution patterns prevent injection attacks
- Comprehensive token handling follows industry best practices  
- Output sanitization ensures sensitive data is protected
- Recent commit #324 further tightens input validation

**No action required for this scan period.**

---

## Appendix

### Threat Model
- Version: 2026-05-18 (initial generation)
- Location: `.factory/threat-model.md`

### Skills Used
- threat-model-generation (for STRIDE analysis)
- commit-security-scan (for vulnerability scanning)
- vulnerability-validation (for finding assessment)

### References
- [CWE Database](https://cwe.mitre.org/)
- [STRIDE Threat Model](https://docs.microsoft.com/en-us/azure/security/develop/threat-modeling-tool-threats)
- [OWASP Top 10](https://owasp.org/www-project-top-ten/)
