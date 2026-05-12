# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **MSRV raised from 1.92 to 1.95.** `[workspace.package] rust-version` updated to `"1.95"`. `rust-toolchain.toml` pins the toolchain to `1.95.0` (minimal profile + rustfmt + clippy). `clippy.toml` sets `msrv = "1.95"`, cognitive-complexity threshold 40, too-many-arguments threshold 8. All CI MSRV references (ci.yml, coverage.yml, release.yml) updated. Documentation and badge updated accordingly.

- **`shipper` crate:** the `shipper-cli` dependency is now behind a default `cli` feature. `cargo install shipper` and the `shipper` binary still work unchanged (the feature is on by default). Library consumers that only want the curated `shipper-core` re-export can opt out with `shipper = { version = "...", default-features = false }`, which drops the `clap` graph. `shipper-core` remains the canonical lean embedding surface.

- **Clippy 1.94/1.95 ratchets activated** ([#191](https://github.com/EffortlessMetrics/shipper/issues/191)). Eight lints moved from `[[planned]]` to `[[active]]` in `policy/clippy-lints.toml` and added to `[workspace.lints.clippy]`: `same_length_and_capacity` (deny), `manual_ilog2`, `decimal_bitwise_operands`, `needless_type_cast`, `manual_checked_ops`, `manual_take`, `unnecessary_trailing_comma`, `disallowed_fields` (deny). All eight had zero workspace fallout at activation. `duration_suboptimal_units` (1.95) stays in `[[planned]]` with measured fallout of 204 sites across 20 files, pending a focused cleanup PR. `manual_pop_if` was listed in the original #191 plan but is not a real clippy lint — `[workspace.lints.rust] unknown_lints = "deny"` rejected it via E0602; the omission is recorded in the ledger.

### Added

- **No-panic baseline detector** ([#187](https://github.com/EffortlessMetrics/shipper/issues/187), PR 8a). `cargo xtask no-panic baseline` walks every tracked production source file (`crates/*/src/**/*.rs`, excluding `tests/`/`benches/`/`examples/` dirs, `tests.rs`/`*_tests.rs` files, and `#[cfg(test)]`/`#[test]` subtrees), classifies every panic-family call site (unwrap, expect, panic, unreachable, todo, unimplemented, index) via `syn`, groups by exact identity (path, family, selector_kind, selector_callee, snippet), and writes the count-keyed result to `policy/no-panic-baseline.json`. The baseline is marked `linguist-generated=true` via `.gitattributes` and receipted in `policy/generated-allowlist.toml`. Current production surface: **59 sites across 28 grouped entries** in 25 files — dominantly `.lock().unwrap()` patterns in `engine/parallel/`, a handful of `serde_json::Value` indexings, and a few plan-side `expect("pkg exists")` invariants.

- **No-panic check + release CI gate** ([#187](https://github.com/EffortlessMetrics/shipper/issues/187), PR 8b). `cargo xtask no-panic check` re-runs the detector, compares the fresh result against `policy/no-panic-baseline.json` keyed by `(path, family, selector_kind, selector_callee, snippet)`, and fails on new entries or count increases. Resolved entries and count decreases are reported as good news but never fail. `--mode advisory` writes a report without bailing (used by `cargo xtask policy-report`, now an eighth area in the unified report). `--mode blocking` (the CLI default) bails non-zero on violations. A new `policy-gate` job in `release.yml` runs `no-panic check --mode blocking` and `check-lint-policy` before `publish-crates-io`, so a tag push cannot publish if either ledger has drifted. Detector ships with 15 unit tests covering the `#[test]`/`#[bench]`/`#[cfg(test)]`/`#[cfg(any(test, ...))]`/`#[cfg(all(..., test))]`/`#[cfg(not(test))]` classifications and the `tests.rs`/`*_tests.rs`/`tests/`/`benches/`/`examples/` filename and directory exclusions. The `cfg(not(test))` case was a regression caught during PR 8b implementation — the naive "any mention of `test` anywhere in the cfg" heuristic would misclassify production-only code as test. Closes #187.

- **Ripr advisory PR lane** ([#182](https://github.com/EffortlessMetrics/shipper/issues/182), thin adoption). Adopts the external [EffortlessMetrics/ripr](https://github.com/EffortlessMetrics/ripr) CLI (`crates.io/crates/ripr` 0.5.0) as a never-blocking PR-time exposure lane. New files: `ripr.toml` at the workspace root (canonical `ripr init` schema with one Shipper override pointing suppressions at `policy/`), `policy/ripr-suppressions.toml` (empty receipt ledger; ripr's required `[[suppression]]` fields plus Shipper's `created`/`review_after` conventions), `.github/workflows/ripr.yml` (advisory job: installs ripr pinned, runs `cargo xtask ripr-pr`, uploads `target/ripr/` artifact, `continue-on-error: true`), `xtask/src/ripr.rs` (thin wrapper that shells out to `ripr pilot --root .` and exits advisory-success when `ripr` is missing locally), and `docs/ci/ripr.md` updated to current ripr terminology (static mutation-exposure analysis, not "reachable mutants not covered"). Shipper consumes ripr; Shipper does not embed RIPR analysis. Mutation-workflow scoping (the second half of #182) stays deferred — `mutation.yml` is still weekly/manual only.

- **Ripr wrapper: `--base` flag + `target/policy/ripr-report.{md,json}` projection** ([#182](https://github.com/EffortlessMetrics/shipper/issues/182), follow-up). `cargo xtask ripr-pr` now accepts `--base <ref>` (defaults to `origin/main`); the flag is forward-looking for the eventual switch from `ripr pilot` to `ripr check --base <ref>`. After ripr writes its native outputs, the wrapper projects `target/ripr/pilot/pilot-summary.md` and `pilot-summary.json` into `target/policy/ripr-report.{md,json}` so the artifact sits alongside other policy reports. The `pilot-summary.json` source is deliberate: it is the ~13 KB compact summary, versus `repo-exposure.json` (~53 MB on the Shipper workspace) and `agent-seam-packets.json` (~34 MB) which are too heavy to republish as a policy artifact. `.github/workflows/ripr.yml` updates its upload glob to `target/ripr/` + `target/policy/ripr-report.*`.

- **Ripr repo-scoped badges** ([#182](https://github.com/EffortlessMetrics/shipper/issues/182), PR 2). Public README badges for `ripr` and `ripr+` are now committed as Shields endpoint JSON under `badges/`. Per upstream ripr policy, README badges are **repo-scoped**, not diff-scoped — a diff badge would read `0` on `main` simply because no diff exists, not because the repo is clean. `cargo xtask repo-ripr-badge-artifacts` runs `ripr check --root . --mode ready --format repo-exposure-json`, extracts `metrics.headline_eligible` (count of repo seams the configured `[severity.seams]` policy treats as non-off), maps the count to a Shields color via `0 -> brightgreen`, `1..=99 -> yellowgreen`, `100..=999 -> orange`, `1000+ -> red`, and writes `badges/ripr.json` + `badges/ripr-plus.json`. Both badges currently project the same count; differentiating `ripr+` to also include test-efficiency findings is upstream territory and deferred. Badges are receipted in `policy/generated-allowlist.toml` and marked `linguist-generated=true` in `.gitattributes`. Refresh cadence is intentionally manual: run the command locally and commit the regenerated badges in their own PR. Initial Shipper count: 2,711 (red).

- **Mutation testing PR-time lane** ([#182](https://github.com/EffortlessMetrics/shipper/issues/182), PR 3). `.github/workflows/mutation.yml` gains a second job, `mutants-pr`, that runs only on PRs carrying the `mutation` or `full-ci` label — explicitly off the default PR hot path. The job invokes `cargo xtask mutants-pr --changed --base origin/<PR-base>`, a new xtask wrapper that computes `git diff <base>...HEAD --name-only -- '*.rs'` (excluding `tests/`/`benches/`), then runs `cargo mutants --no-shuffle --file <each>` against only those files. `--dry-run` maps to `cargo mutants --list` for local shape inspection. If `cargo-mutants` is missing locally the wrapper exits advisory-success; CI installs it before invoking. The pre-existing weekly job stays unchanged (still covers `shipper-duration` / `shipper-types` / `shipper-config`) — expanding it to the full trust-critical surface is too expensive for a 60-minute job and is its own future rollout step. `docs/ci/test-evidence-lanes.md` updated to reflect the new lane.

### Planned — Rust 1.95 / 0.4.0 Quality Rollout

The next release line is **0.4.0** (minor bump because MSRV increases). The rollout is tracked in [`docs/ci/rust-1.95-rollout.md`](docs/ci/rust-1.95-rollout.md) and proceeds through fifteen PRs:

- Raise MSRV from Rust 1.92 to Rust 1.95.
- Pin toolchain via `rust-toolchain.toml`.
- Add `xtask` Rust-native policy runner.
- Add Clippy lint ledger (`policy/clippy-lints.toml`) and checker.
- Activate Rust 1.95 compiler lint floor (`unsafe_op_in_unsafe_fn`, `unused_must_use`, `const_item_interior_mutations`, et al.).
- Activate Rust 1.95 Clippy ratchets (`manual_checked_ops`, `manual_take`, `unnecessary_trailing_comma`, `disallowed_fields`, et al.). `duration_suboptimal_units` deferred; `manual_pop_if` from the original plan turned out not to be a real clippy lint.
- Add exact no-panic no-new-debt baseline and checker.
- Add non-Rust file policy allowlists for workflows, generated files, executables, dependency surfaces.
- Add advisory `ripr` PR-time exposure lane.
- Add scoped CI lane policy and targeted mutation routing.
- Apply Rust 1.95 API cleanup in publish and receipt paths.
- First Clippy / no-panic debt burndown.
- Version alignment to `0.4.0-rc.1` / `v0.4.0`.
- Release dry-run proof.

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
