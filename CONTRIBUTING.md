# Contributing to Shipper

Thank you for your interest in contributing to Shipper! This document provides guidelines and instructions for contributing.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Environment](#development-environment)
- [Making Changes](#making-changes)
- [Testing](#testing)
- [Pull Request Process](#pull-request-process)
- [Code Style](#code-style)

---

## Code of Conduct

We follow the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct). Please be respectful and constructive in all interactions.

---

## Getting Started

Active development happens in
[`EffortlessMetrics/shipper-swarm`](https://github.com/EffortlessMetrics/shipper-swarm).
The original [`EffortlessMetrics/shipper`](https://github.com/EffortlessMetrics/shipper)
repository remains the release authority for crates.io publishing and release
evidence until that authority is explicitly moved.

Do not add crates.io publish tokens, release signing secrets, or release
workflow credentials to `shipper-swarm`.

The operating policy is documented in
[docs/status/SWARM_OPERATION.md](docs/status/SWARM_OPERATION.md):

- PRs into `shipper-swarm/main` are squash-merged.
- Promotion from `shipper-swarm/main` back to `shipper/main` uses a merge
  commit, never squash or rebase.
- `shipper-swarm/main` must remain a continuation of `shipper/main` history.
- Source-repo sync and credential rules are mirrored in
  [docs/status/SWARM_SYNC.md](docs/status/SWARM_SYNC.md).

1. Fork the development repository.
2. Clone your fork:
   ```bash
   git clone https://github.com/YOUR_USERNAME/shipper-swarm.git
   cd shipper-swarm
   ```
3. Install Changie v1.25.1 and the repository-owned local pre-commit hook:
   ```bash
   changie --version
   cargo precommit install
   cargo precommit status
   ```
4. Create a branch for your changes:
   ```bash
   git checkout -b my-feature
   ```

---

## Development Environment

### Prerequisites

- **Rust**: 1.95 or later (check with `rustc --version`)
- **Git**: For version control
- **Changie**: v1.25.1 for local change-fragment authoring and pre-commit validation
- **cargo-nextest** (optional): For better test output

### Building

```bash
# Build all crates
cargo build --workspace

# Build in release mode
cargo build --workspace --release
```

### Running

```bash
# Run the CLI locally
cargo run --package shipper -- <command>

# Example
cargo run --package shipper -- plan --help
```

---

## Making Changes

### Before You Start

- Check the [development issues](https://github.com/EffortlessMetrics/shipper-swarm/issues)
  and open PRs for related work before claiming a slice. Start with the
  [build-readiness handoff](plans/swarm/build-readiness.md) for the audited work
  order and the current release-freeze boundary.
- Open routine product, test, documentation, and CI work in `shipper-swarm`.
  Existing linked [source-repository issues](https://github.com/EffortlessMetrics/shipper/issues)
  retain their context; implement routine development through swarm PRs and link
  the original issue. Promotion, rehearsal, publication, and release evidence
  remain owned by the release-authority issues.
- For significant changes, open an issue first to discuss the approach.
- Keep changes focused and atomic.

### Changelog fragments

Shipper uses Changie as a **local pre-commit authoring contract**, not a CI gate.
For user-visible, compatibility-relevant, security, recovery, or operational
changes, create and stage a fragment with the implementation:

```bash
changie new
```

The resulting YAML file lives under `.changes/unreleased/`. Choose its kind,
primary audience, and release-note importance deliberately so the release-prep
campaign can promote headline changes, retain useful detail, and demote
maintenance work without reconstructing intent months later.

The repository-owned hook validates the staged Git index, checks that relevant
changes have a branch-local fragment, validates Changie v1.25.1 with a dry-run
batch, and writes `target/hooks/pre-commit.json`:

```bash
cargo precommit
```

Test-only, snapshot-only, ordinary CI, policy, agent-control, and internal status
changes are exempt by path. For an unusual behavior-preserving change inside a
normally user-facing source path, set
`SHIPPER_PRECOMMIT_CHANGELOG_EXEMPT` to a substantive reason of at least 12
characters for that commit; the reason is retained in the local receipt and
should be stated in the PR. See [.changes/README.md](.changes/README.md) for the
full workflow.

No GitHub Actions workflow installs or runs Changie. The hook is bypassable and
is not merge authority; normal hosted Rust, policy, and review gates remain
separate.

### Code Organization

| Directory | Purpose |
|-----------|---------|
| `crates/shipper/` | Install facade and curated library re-export |
| `crates/shipper-cli/` | CLI adapter: clap, subcommands, help, human/JSON output |
| `crates/shipper-core/` | Engine/library implementation |
| `docs/` | User documentation |
| `templates/` | CI/CD templates |
| `fuzz/` | Fuzzing targets |

### Key Modules

| Module | Responsibility |
|--------|----------------|
| `crates/shipper-core/src/plan/` | Publish planning and ordering |
| `crates/shipper-core/src/engine/` | Publish/preflight/resume execution engine |
| `crates/shipper-registry/src/` | Registry API interactions |
| `crates/shipper-core/src/ops/cargo/` | Cargo command wrappers |
| `crates/shipper-core/src/state/` | State persistence |
| `crates/shipper-core/src/state/events/` | Event logging |
| `crates/shipper-config/` | Configuration handling |

---

## Testing

### GitHub Actions validation

Run the pinned workflow linter from the repository root before handing off
workflow changes:

```bash
uvx --from jactionlint==2.5.0 jactionlint --profile correctness --no-online --shellcheck=""
```

If installed with `uv tool install jactionlint==2.5.0`, the equivalent command is
`jactionlint --profile correctness --no-online --shellcheck=""`; `jactionlint --version`
shows the selected version. The `Runner Routing Guard` uses the same correctness
profile and checks a SHA256-pinned release binary. It keeps the pre-existing
ShellCheck-disabled scope and reads `.github/actionlint.yaml` for runner labels.
The gate is static workflow proof, not evidence of runner availability, release
success, or cold/warm-cache timings. YAML parsing alone does not replace it.

### Running Tests

The [Testing Guide](docs/testing.md) explains package selection, focused tests,
and configured CI proof. At the workspace root, plain `cargo test` selects only
the `shipper` and `shipper-cli` default members; it does not select the engine,
support crates, or xtask for their own tests.

```bash
# Run workspace tests and doc tests with all features
cargo test --workspace --all-features --locked

# Quick check of the two default members
cargo test --locked

# Run a specific CLI integration test binary
cargo test --package shipper-cli --test cli_e2e --locked

# Run with verbose output
cargo test --workspace --all-features --locked -- --nocapture

# Run library unit tests only (skip integration tests and doc tests)
cargo test --workspace --lib --all-features --locked
```

### Test Categories

| Type | Location | Purpose |
|------|----------|---------|
| Unit tests | `src/**/tests` modules | Test individual functions |
| Integration tests | `tests/` directories | Test module interactions |
| E2E tests | `crates/shipper-cli/tests/cli_e2e.rs` | Test CLI behavior |
| BDD tests | `crates/shipper-cli/tests/bdd_*.rs` | Behavior-driven scenarios |
| Property tests | Throughout using proptest | Property-based testing |

### Writing Tests

- Place unit tests in `#[cfg(test)]` modules within source files.
- Place integration tests in the `tests/` directory.
- Use descriptive test names: `given_X_when_Y_then_Z`.
- Add property tests for complex logic using `proptest`.

---

## Pull Request Process

### Before Submitting

1. **Run the staged local gate:**
   ```bash
   cargo precommit
   ```

2. **Format your code:**
   ```bash
   cargo fmt
   ```

3. **Run clippy:**
   ```bash
   cargo clippy --workspace -- -D warnings
   ```
   All warnings must be resolved.

4. **Run all tests:**
   ```bash
   cargo test --workspace
   ```

5. **Update documentation** if your changes affect user-facing behavior.

6. **Check the Changie disposition:** include a fragment for a relevant change,
   or state the explicit exemption reason in the PR.

### PR Guidelines

- **Title**: Use conventional commit format
  - `feat: add shell completion support`
  - `fix: handle missing registry gracefully`
  - `docs: update configuration examples`
  - `refactor: simplify publish loop`

- **Description**: Explain what and why, not how.
- **Link issues**: Reference any related issues.
- **Small PRs**: Keep changes focused and reviewable.
- **Changelog disposition**: identify the fragment or the reason no fragment is needed.
- **Review identity**: record the intended base, exact head, base SHA,
  merge-base SHA, and any synthetic merge/check commit used for final proof.
- **Review map**: name semantic owners, callers/consumers, highest-risk
  invariants, required adversarial/platform proof, and schema/docs/package/
  support/release impact.
- **Required gate**: `shipper-swarm/main` requires `Shipper Rust Small Result`;
  do not require route-specific implementation jobs directly because only one
  route runs per attempt.
- **Merge method**: use squash merge for normal `shipper-swarm` PRs.
  Source-backfill PRs that merge release-authority commits from `shipper/main`
  are the exception and must use merge commits.

### Review Process

A PR needs both a substantive **candidate judgment** and a separate live
**integration posture**. An approval or green check list alone is not enough.

1. Reconstruct the cumulative claim, non-goals, controlling authority, exact
   head/base/merge-base identities, changed semantic surfaces, proof, prior
   findings, and limitations.
2. Apply proportionate correctness, architecture, integration, test-oracle,
   security/release/claim-boundary, and simplification passes. Inspect relevant
   callers, consumers, schemas, fixtures, packages, docs, and workflows beyond
   the changed lines.
3. Post precise inline findings where GitHub permits. Avoid duplicate threads.
   A clean review records inspected surfaces, challenge passes, residual risk,
   evidence provenance, and what remains unproved; a generic `LGTM` is not
   review evidence.
4. Return one substantive result:

   ```text
   REVIEW_CURRENT
   CHANGES_REQUIRED
   NOT_PROVEN
   BLOCKED_BY_PREREQUISITE
   SUPERSEDED_OR_CLOSE
   ```

5. After a repair, rerun affected proof and re-review affected findings,
   semantic dimensions, and repair-created edge cases. A fixing commit or
   evidence-backed rejection must be replied to the relevant thread before it
   is resolved; blanket automated resolution is forbidden.
6. Only a current `REVIEW_CURRENT` result proceeds to live integration
   evaluation. Classify checks and merge state separately as:

   ```text
   INTEGRATION_READY
   PR_IN_FLIGHT
   MERGE_BLOCKED
   NOT_PROVEN
   ```

7. Merge only when the reviewed effective subject remains current, required
   checks are terminal and successful or explicitly not applicable,
   substantive threads are resolved with evidence, and the PR is actually
   mergeable. Squash-merge normal development PRs; use the separate
   history-preserving contract for source-backfill PRs.
8. Verify merged `main`, reconcile the controlling issue and dependent PRs,
   and retain the review/proof record after merge.

Claude Code and Codex execute this lifecycle through their own complete native
skills under `.claude/skills/` and `.agents/skills/`. Shared semantics are in
[docs/agent-context/review-currentness.md](docs/agent-context/review-currentness.md),
but that document is not an executable review authority. Every PR in a stack
must receive its own review before a campaign-level synthesis.

---

## Code Style

### Formatting

- Use `cargo fmt` before committing.
- Maximum line length: 100 characters (rustfmt default).

### Naming Conventions

| Item | Convention | Example |
|------|------------|---------|
| Types | PascalCase | `PublishPlan` |
| Functions | snake_case | `build_plan()` |
| Constants | SCREAMING_SNAKE | `MAX_RETRIES` |
| Modules | snake_case | `engine_parallel` |

### Documentation

- Add rustdoc comments (`///`) for public items.
- Include examples in doc comments when helpful.
- Keep line comments (`//`) for implementation notes.

### Error Handling

- Use `Result<T, E>` for fallible operations.
- Use `thiserror` for custom error types.
- Provide actionable error messages.

### Commit Messages

Follow conventional commits:

```
<type>: <description>

[optional body]

[optional footer]
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`

---

## Questions?

- Open a [discussion](https://github.com/effortlessmetrics/shipper/discussions)
  for questions. Discussions remain in the release-authority repo until
  explicitly moved.
- Open a [development issue](https://github.com/EffortlessMetrics/shipper-swarm/issues)
  for routine bugs or features. Promotion, rehearsal, publication, and release
  evidence retain the source-repository issue owners described in
  [Before You Start](#before-you-start).

Thank you for contributing!
