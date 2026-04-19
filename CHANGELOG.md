# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0-rc.2] - 2026-04-18

Nine-competency roadmap ([#109](https://github.com/EffortlessMetrics/shipper/issues/109)) landed end-to-end on `main` since `v0.3.0-rc.1`: **Prove**, **Survive**, **Reconcile**, **Narrate**, **Remediate**, **Harden** (Trusted Publishing), **Ergonomics** (three-crate split), plus consistency enforcement and operator-trust docs.

### Added

#### Reconcile ([#99](https://github.com/EffortlessMetrics/shipper/issues/99))

- **Ambiguous-publish reconciliation against registry truth.** When `cargo publish` exits ambiguously, Shipper now polls the registry (sparse index + API per config) instead of blind-retrying. Outcomes: `Published` (skip retry), `NotPublished` (safe retry), `StillUnknown` (halt for operator). Cargo stdout is demoted to a fast-path hint; registry is authoritative. Resume-path reconciles `Ambiguous` state before re-entering the retry loop.
- **Events:** `PublishReconciling { method }`, `PublishReconciled { outcome }`.
- **State:** `PackageState::Ambiguous { message }` persists across resume.
- **BDD scenarios** for all three outcomes (Published, NotPublished, StillUnknown) plus resume-from-Ambiguous.

#### Prove ([#97](https://github.com/EffortlessMetrics/shipper/issues/97))

- **`shipper rehearse`** — phase-2 preflight: publish every crate to a non-crates.io rehearsal registry, verify visibility, then optionally smoke-install before live dispatch. `engine::run_rehearsal` is the programmatic entry point.
- **Hard gate:** `run_publish` can require a passing rehearsal receipt before dispatching to production (configurable).
- **Smoke-install** step validates the rehearsal artifact actually resolves and builds as a dependency.

#### Remediate ([#98](https://github.com/EffortlessMetrics/shipper/issues/98))

- **`shipper yank <crate>@<version>`** — receipt-driven yank with event emission (`PackageYanked`).
- **`shipper plan-yank`** — generates a reverse-topological containment plan from a receipt; `--starting-crate` supports graph-mode containment; `--plan <file>` executes a saved plan.
- **`--mark-compromised`** and **`shipper fix-forward`** — plan a minimal repair for a partial release; receipt schema carries `compromised_at`, `compromised_by`, `superseded_by` fields.

#### Harden ([#96](https://github.com/EffortlessMetrics/shipper/issues/96))

- **Trusted Publishing (OIDC)** for crates.io: first-class support in the publish flow and CI templates. Tokens are no longer the only supported path.

#### Narrate ([#91](https://github.com/EffortlessMetrics/shipper/issues/91))

- **Retry visibility** — structured `RetryBackoff` events and live CLI narration so operators can see what the engine is waiting on and why.

#### Survive ([#94](https://github.com/EffortlessMetrics/shipper/issues/94))

- **crates.io-aware backoff** — registry-aware rate-limit detection uses `crate_exists` to distinguish new-crate throttling from transient failures.

#### Recover ([#90](https://github.com/EffortlessMetrics/shipper/issues/90))

- **Synthetic rehearsal test** and **operator rehearsal playbook** proving interruption-resume behavior for preflight, backoff, and partial-publish phases.

### Changed

#### Packaging — three-crate product shape ([#95](https://github.com/EffortlessMetrics/shipper/issues/95))

- **`shipper-core`** (new) — engine library with no CLI dependencies. Stable embedding surface: `plan`, `preflight`, `publish`, `resume`, `reconcile`, `rehearsal`, `remediate`, state/events/receipts, policy/readiness.
- **`shipper-cli`** — promoted from placeholder to real CLI adapter. Owns `clap` parsing, subcommand dispatch, help text, progress rendering. Exposes `pub fn run() -> anyhow::Result<()>` as the embedding entry point.
- **`shipper`** — shrunk to install façade. 3-line binary forwarding to `shipper_cli::run()`, plus a library re-exporting a curated subset of `shipper-core`. **This is the recommended install path:** `cargo install shipper --locked`.
- **Backward compatibility:** `cargo install shipper-cli --locked` still works; the old `shipper-cli` binary forwards to the same `run()`.

#### Consistency ([#93](https://github.com/EffortlessMetrics/shipper/issues/93))

- **Events-as-truth invariant** now enforced at end-of-run. `events.jsonl` is authoritative; `state.json` is a projection; `receipt.json` is a summary. Drift is detected and reported via `StateEventDriftDetected`.

#### Preflight ([#92](https://github.com/EffortlessMetrics/shipper/issues/92))

- **Workspace-verify event** slimmed; ANSI stripped from captured output; full verify log written to a sidecar file rather than inline events.

### Fixed

- **Resume:** `PackageSkipped` event now emits correctly when resume finds a package already in terminal state.

### Documentation

- **Operator-trust pack:** `not_proven` explainer, stalled-run triage, state-files cheat sheet.
- **Roadmap aligned** with mission/steering docs; Diátaxis reorganization (tutorials, how-to, reference, explanation).
- **Docs demote cargo stdout to hint**; registry truth is authoritative for safety-critical decisions.
- Three-crate split reflected across README, runbook, examples, CI templates, `docs/structure.md`, `docs/architecture.md`, GEMINI.md, Copilot instructions.

### Install

```bash
# New recommended path
cargo install shipper --locked

# Backward-compatible (same code path)
cargo install shipper-cli --locked
```

Embedders who want a clap-free library surface should depend on `shipper-core` directly.

## [0.3.0-rc.1] - 2026-02-27

### Added

- **Multi-Registry Publishing**: Publish crates to multiple registries in a single command (`--all-registries` or `--registries name1,name2`).
- **Sparse Index Caching**: High-performance ETag-based disk caching for Cargo sparse index fragments, significantly accelerating readiness polling and reducing bandwidth.
- **Selective Resume**: New `--resume-from <package>` flag to start publishing from a specific crate in the plan.
- **Enhanced Diagnostics**: Substantially improved `shipper doctor` with network reachability checks, permission validation, and git context detection.
- **Deep Dry-Run Visibility**: Preflight reports now capture and display detailed stdout/stderr from cargo dry-run failures.
- **Global Quiet Mode**: New global `--quiet` flag for cleaner CI/CD logs.
- **Shell Completions**: Support for generating shell completion scripts via `shipper completion <shell>`.
- **New CI Templates**: Added Azure DevOps and CircleCI workflow snippets (`shipper ci azure-devops`, `shipper ci circleci`).

### Changed

- **Granular Locking**: Moved from global file locking to workspace-aware locking using path hashing, allowing parallel publishes of different workspaces.
- **Atomic State Operations**: Improved robustness of lock and state file writes using atomic filesystem operations.
- **CI Progress Reporting**: Optimized progress reporter to emit one-line status updates in non-TTY environments.
- **Expanded Error Classification**: Added dozens of new patterns to `shipper-cargo-failure` for more accurate retryable vs. permanent detection.
- **Config Schema Tracking**: Added `schema_version` validation to `.shipper.toml` for future migration support.

### Fixed

- Fixed flaky BDD publish tests by introducing more robust mock server synchronization.
- Fixed race conditions in lock acquisition.
- Resolved clippy warnings across the workspace.

## [0.2.0] - 2026-02-14

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
- `shipper config init` - Generate a default `.shipper.toml` configuration file
- `shipper config validate` - Validate a configuration file

#### New CLI Flags

- `--config <path>` - Path to a custom `.shipper.toml` configuration file
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

- Configuration file support (`.shipper.toml`) with `config init` and `config validate` subcommands
- Lock file mechanism to prevent concurrent publish operations
- Configurable evidence capture with adjustable output line limits
- JSON output format for CI/CD integration
- Readiness verification with multiple methods (API, index, combined)
- Publish policies for different safety levels
- Enhanced receipt format with embedded evidence
- Schema versioning for state, plan, and receipt files

#### Parallel Publishing

- **Parallel publishing**: Packages at the same dependency level can now be published concurrently with `--parallel`
- New CLI flags: `--parallel`, `--max-concurrent <N>`, `--per-package-timeout <duration>`
- Configurable via `[parallel]` section in `.shipper.toml`

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

## [0.1.0] - 2025-01-15

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
