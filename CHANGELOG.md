# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2024-02-10

### Added

#### Four Pillars of Publishing Reliability

- **Evidence Capture**: Every publish operation now captures detailed evidence including stdout, stderr, exit codes, and timestamps for debugging and auditing purposes.
- **Event Logging**: Comprehensive event log (`events.jsonl`) records every step of the publishing process with timestamps for complete audit trails.
- **Readiness Checks**: Configurable readiness verification ensures published crates are actually available on the registry before proceeding.
- **Publish Policies**: Three built-in policies control verification behavior (safe, balanced, fast) allowing users to choose the right balance of safety and speed.

#### New CLI Commands

- `shipper inspect-events` - View detailed event log with timestamps and evidence
- `shipper inspect-receipt` - View detailed receipt with captured evidence
- `shipper ci github-actions` - Print GitHub Actions workflow snippet
- `shipper ci gitlab` - Print GitLab CI workflow snippet
- `shipper clean` - Clean state files (state.json, receipt.json, events.jsonl)

#### New CLI Flags

- `--policy <policy>` - Publish policy: safe (verify+strict), balanced (verify when needed), fast (no verify)
- `--verify-mode <mode>` - Verify mode: workspace (default), package (per-crate), none (no verify)
- `--readiness-method <method>` - Readiness check method: api (default, fast), index (slower, more accurate), both (slowest, most reliable)
- `--readiness-timeout <duration>` - How long to wait for registry visibility during readiness checks (default: 5m)
- `--readiness-poll <duration>` - Poll interval for readiness checks (default: 2s)
- `--no-readiness` - Disable readiness checks (for advanced users)
- `--output-lines <number>` - Number of output lines to capture for evidence (default: 50)
- `--format <format>` - Output format: text (default) or json
- `--force` - Force override of existing locks (use with caution)
- `--lock-timeout <duration>` - Lock timeout duration (default: 1h)

#### New State Files

- `events.jsonl` - Line-delimited JSON event log for debugging and auditing

#### New Features

- Lock file mechanism to prevent concurrent publish operations
- Configurable evidence capture with adjustable output line limits
- JSON output format for CI/CD integration
- Readiness verification with multiple methods (API, index, combined)
- Publish policies for different safety levels
- Enhanced receipt format with embedded evidence

### Changed

- Improved error messages with context and evidence references
- Enhanced state file format with additional metadata
- Better handling of registry API rate limits
- Improved retry logic with exponential backoff and jitter

### Fixed

- Fixed potential race conditions in state file handling
- Improved handling of ambiguous failures where upload may have succeeded
- Better error recovery for network timeouts
- Fixed issues with resume when workspace configuration changes

### Breaking Changes

- The state file format has changed. Previous versions of shipper cannot resume from v0.2 state files.
- The receipt file format has been enhanced with additional evidence fields.
- Default readiness timeout increased from 2m to 5m for more reliable verification.

### Migration Guide from v0.1.0

If you're upgrading from v0.1.0:

1. **Clean old state files**: Run `shipper clean` before upgrading to remove old state files.
2. **Update CI workflows**: The new `shipper ci` command can generate updated workflow snippets.
3. **Review readiness settings**: The default readiness timeout has increased; adjust if needed.
4. **Test publish policies**: Try the different policy modes to find the best fit for your workflow.

## [0.1.0] - 2024-01-XX

### Added

- Initial release
- Basic publish planning and execution
- Preflight checks (git cleanliness, publishability, registry reachability)
- Optional ownership/permissions verification
- Retry/backoff for retryable failures
- Registry API verification before declaring success
- Resumable execution with state persistence
- Status command to compare local versions to registry
- Doctor command for environment and auth diagnostics

[0.2.0]: https://github.com/yourusername/shipper/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/yourusername/shipper/releases/tag/v0.1.0
