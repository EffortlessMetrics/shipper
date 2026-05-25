# Shipper Threat Model

**Generated:** 2026-05-25
**Repository:** EffortlessMetrics/shipper
**Version:** 1.0

## Scope

This threat model covers the shipper publishing tool ecosystem:
- `shipper-core` — engine/library logic
- `shipper-cli` — CLI adapter
- `shipper` — install façade

## System Overview

Shipper is a Cargo workspace publishing tool that orchestrates multi-crate publishing to registries (primarily crates.io). The core flow is: **plan → preflight → publish → resume**.

### Key Components

1. **Auth Module** (`ops/auth/`) — Token resolution from env vars and credentials files
2. **Cargo Module** (`ops/cargo/`) — `cargo metadata` and `cargo publish` invocation
3. **Lock Module** (`ops/lock/`) — Advisory file-based locking for concurrent operation prevention
4. **Engine** — Orchestrates the publish pipeline with retry/backoff
5. **State Store** — Persists execution state to `.shipper/` directory

## STRIDE Analysis

### Spoofing

| Threat | Mitigations |
|--------|-------------|
| Impersonating a registry | Tokens are validated via cargo; TLS for all registry communication |
| Token theft via environment | Tokens stored in env vars, not logged; `shipper_output_sanitizer` redacts in all output |

### Tampering

| Threat | Mitigations |
|--------|-------------|
| Corrupt lock file | JSON parse errors detected; `acquire_with_timeout` removes corrupt locks |
| State file manipulation | Events are append-only; state is a projection, events are authoritative |
| Tamper with cargo output | All output redacted before logging |

### Repudiation

| Threat | Mitigations |
|--------|-------------|
| No proof of publish | Events.jsonl is append-only; receipt.json provides audit trail |
| Token source ambiguity | `TokenSource` enum tracks origin (EnvDefault, EnvRegistry, CredentialsFile) |

### Information Disclosure

| Threat | Mitigations |
|--------|-------------|
| Token in logs | `redact_sensitive()` and `tail_lines()` sanitize all cargo output |
| Credential file access | Only reads from `$CARGO_HOME/credentials.toml` |
| Git remote with embedded token | Detected and redacted in git context collection |

### Denial of Service

| Threat | Mitigations |
|--------|-------------|
| Lock contention | Stale lock detection with configurable timeout |
| Cargo publish timeout | 100ms polling loop with SIGKILL on deadline |
| Registry rate limiting | Backoff retry with `ErrorClass::Retryable` classification |

### Elevation of Privilege

| Threat | Mitigations |
|--------|-------------|
| N/A — shipper runs with user's existing cargo permissions | N/A |

## Known Limitations

1. **Lock race condition**: `LockFile::acquire` uses check-then-create (TOCTOU). Under tight concurrent contention, more than one process may acquire the lock. This is documented and considered acceptable for the advisory lock use case.

2. **No OS-level file locking**: Lock relies on cooperative workspace behavior, not mandatory file locking.

## Security Controls

- `unsafe_code = "forbid"` enforced workspace-wide
- Tokens are opaque strings; never logged
- Whitespace-trimmed empty tokens treated as absent
- OIDC detection requires both `ACTIONS_ID_TOKEN_REQUEST_URL` and `ACTIONS_ID_TOKEN_REQUEST_TOKEN`
- All cargo output passed through `shipper_output_sanitizer::tail_lines` before logging
