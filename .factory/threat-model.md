# Threat Model

**Repository:** EffortlessMetrics/shipper
**Generated:** 2026-06-01
**Tool:** cargo publish wrapper for crates.io workspaces

## Overview

Shipper is a Rust CLI tool that orchestrates publishing crates to crates.io registries. It handles multi-crate workspace publishing with retry logic, state management, and optional webhook notifications.

### Architecture

```
shipper (binary facade)
  └── shipper-cli (CLI adapter with clap)
       └── shipper-core (engine - library logic)
```

- **shipper-core**: Engine crate with no CLI dependencies
- **shipper-cli**: CLI adapter with clap/indicatif
- **shipper**: Install facade with re-exports

##STRIDE Analysis

### Spoofing

| Threat | Description | Mitigation |
|--------|-------------|------------|
| Token theft | Attacker steals registry tokens from env/credentials | Tokens are opaque strings, never logged (mask_token() shows only 4+4 chars) |
| OIDC impersonation | Fake OIDC tokens from GitHub Actions | Requires both ACTIONS_ID_TOKEN_REQUEST_URL AND ACTIONS_ID_TOKEN_REQUEST_TOKEN |
| Credential file tampering | Malicious modification of credentials.toml | File permissions are OS-dependent; user responsibility |

### Tampering

| Threat | Description | Mitigation |
|--------|-------------|------------|
| State file corruption | Malicious modification of state.json | Atomic writes via temp+rename with fsync |
| Event log injection | Injecting fake events into events.jsonl | Append-only; validate schema on read |
| Lock file DoS | Deleting/modifying .shipper/lock | Advisory lock with stale detection |
| Config tampering | Modifying .shipper.toml | TOML schema validation; CLI overrides |

### Repudiation

| Threat | Description | Mitigation |
|--------|-------------|------------|
| No cryptographic signing | Events not cryptographically signed | PID/hostname/timestamp recorded; future: add HMAC |
| Audit trail gaps | Missing events in event log | events.jsonl is append-only; state.json is projection |

### Information Disclosure

| Threat | Description | Mitigation |
|--------|-------------|------------|
| Token exposure in logs | Accidental token logging | mask_token() redacts tokens |
| Webhook secret in memory | Secrets held in memory during webhooks | skip_serializing_if prevents serialization |
| Config exposure | Sensitive config values in .shipper.toml | User responsibility to exclude from VCS |
| Encryption passphrase in env | Passphrase via env var | No logging of passphrase values |

### Denial of Service

| Threat | Description | Mitigation |
|--------|-------------|------------|
| Registry rate limiting | HTTP 429 from crates.io | Exponential backoff (1s base, 60s max) |
| Lock file starvation | Process holds lock indefinitely | Stale lock detection with configurable timeout |
| Disk space exhaustion | Large events.jsonl | User responsibility for disk management |
| Webhook timeout | Slow/unresponsive webhook endpoint | Configurable timeout (default 30s) |

### Elevation of Privilege

| Threat | Description | Mitigation |
|--------|-------------|------------|
| Unsafe code execution | Arbitrary code via unsafe blocks | unsafe_code = "forbid" workspace-wide |
| Shell injection | Command injection via subprocess | std::process::Command with explicit args array (no shell) |
| Path traversal | Access outside .shipper/ directory | Restricted to state-dir; no user-controlled paths |

## Security Boundaries

```
User Environment          Shipper Process         Registry API
─────────────────         ───────────────         ────────────
CARGO_REGISTRY_TOKEN ────> Token Resolution ────> cargo publish
     │                          │
     │                          v
     │                   mask_token() ──> Log output (redacted)
     │
     v
.shipper/  <────────>  State Files
     │
     ├── state.json (projection)
     ├── events.jsonl (authoritative)
     ├── receipt.json (summary)
     └── lock (advisory)
```

## Key Data Flows

### Token Resolution
1. Check CARGO_REGISTRY_TOKEN env var
2. Check CARGO_REGISTRIES_{NAME}_TOKEN env var
3. Read from $CARGO_HOME/credentials.toml
4. If OIDC detected (ACTIONS_ID_TOKEN_REQUEST_*), request token from GitHub Actions OIDC
5. Return first valid token found

### Publish Flow
1. Plan: Generate topological sort of crates
2. Preflight: Validate git, registry reachability, dry-run
3. Publish: Execute cargo publish per crate with retry
4. Verify: Check registry visibility (API or sparse index)
5. State: Persist after each step for resumability

### Encryption Flow (Webhooks)
1. Derive key: PBKDF2(passphrase, salt, 100k iterations) → 256-bit key
2. Encrypt: AES-256-GCM(nonce, aad, plaintext) → ciphertext + tag
3. Transmit: HMAC-SHA256(secret, payload) for integrity
4. Signature header: X-Hub-Signature-256: sha256=HMAC

## Mitigation Recommendations

| Priority | Recommendation |
|----------|----------------|
| High | Maintain unsafe_code = "forbid" workspace-wide |
| High | Continue token masking in all logging paths |
| Medium | Consider adding HMAC signatures to events.jsonl |
| Medium | Add filesystem permissions checks for credentials |
| Low | Document security assumptions in SECURITY.md |

## References

- [CWE Database](https://cwe.mitre.org/)
- [STRIDE Threat Model](https://docs.microsoft.com/en-us/azure/security/develop/threat-modeling-tool-threats)
- [crates.io API](https://crates.io/api)
