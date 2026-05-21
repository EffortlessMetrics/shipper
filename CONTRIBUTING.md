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

1. Fork the development repository
2. Clone your fork:
   ```bash
   git clone https://github.com/YOUR_USERNAME/shipper-swarm.git
   cd shipper-swarm
   ```
3. Create a branch for your changes:
   ```bash
   git checkout -b my-feature
   ```

---

## Development Environment

### Prerequisites

- **Rust**: 1.95 or later (check with `rustc --version`)
- **Git**: For version control
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

- Check existing [issues](https://github.com/effortlessmetrics/shipper/issues)
  for related work. Issue tracking remains in the release-authority repo until
  it is explicitly moved.
- For significant changes, open an issue first to discuss the approach
- Keep changes focused and atomic

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
| `crates/shipper-core/src/registry/` | Registry API interactions |
| `crates/shipper-core/src/cargo.rs` | Cargo command wrappers |
| `crates/shipper-core/src/state/` | State persistence |
| `crates/shipper-core/src/events.rs` | Event logging |
| `crates/shipper-config/` | Configuration handling |

---

## Testing

### Running Tests

```bash
# Run all tests
cargo test --workspace

# Run specific test
cargo test --package shipper --test test_name

# Run with verbose output
cargo test --workspace -- --nocapture

# Run only unit tests (skip E2E)
cargo test --package shipper
```

### Test Categories

| Type | Location | Purpose |
|------|----------|---------|
| Unit tests | `src/**/tests` modules | Test individual functions |
| Integration tests | `tests/` directories | Test module interactions |
| E2E tests | `crates/shipper-cli/tests/cli_e2e.rs` | Test CLI behavior |
| BDD tests | `crates/shipper-cli/tests/implementation_plan_bdd.rs` | Behavior-driven scenarios |
| Property tests | Throughout using proptest | Property-based testing |

### Writing Tests

- Place unit tests in `#[cfg(test)]` modules within source files
- Place integration tests in the `tests/` directory
- Use descriptive test names: `given_X_when_Y_then_Z`
- Add property tests for complex logic using `proptest`

---

## Pull Request Process

### Before Submitting

1. **Format your code:**
   ```bash
   cargo fmt
   ```

2. **Run clippy:**
   ```bash
   cargo clippy --workspace -- -D warnings
   ```
   All warnings must be resolved.

3. **Run all tests:**
   ```bash
   cargo test --workspace
   ```

4. **Update documentation** if your changes affect user-facing behavior.

### PR Guidelines

- **Title**: Use conventional commit format
  - `feat: add shell completion support`
  - `fix: handle missing registry gracefully`
  - `docs: update configuration examples`
  - `refactor: simplify publish loop`

- **Description**: Explain what and why, not how
- **Link issues**: Reference any related issues
- **Small PRs**: Keep changes focused and reviewable
- **Required gate**: `shipper-swarm/main` requires `Shipper Rust Small Result`;
  do not require route-specific implementation jobs directly because only one
  route runs per attempt.
- **Merge method**: use squash merge for normal `shipper-swarm` PRs.

### Review Process

1. All PRs require at least one approval
2. CI must pass (tests, clippy, fmt)
3. Address review feedback promptly
4. Squash-merge into `shipper-swarm/main`

---

## Code Style

### Formatting

- Use `cargo fmt` before committing
- Maximum line length: 100 characters (rustfmt default)

### Naming Conventions

| Item | Convention | Example |
|------|------------|---------|
| Types | PascalCase | `PublishPlan` |
| Functions | snake_case | `build_plan()` |
| Constants | SCREAMING_SNAKE | `MAX_RETRIES` |
| Modules | snake_case | `engine_parallel` |

### Documentation

- Add rustdoc comments (`///`) for public items
- Include examples in doc comments when helpful
- Keep line comments (`//`) for implementation notes

### Error Handling

- Use `Result<T, E>` for fallible operations
- Use `thiserror` for custom error types
- Provide actionable error messages

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
- Open an [issue](https://github.com/effortlessmetrics/shipper/issues) for bugs
  or features. Issues remain in the release-authority repo until explicitly
  moved.

Thank you for contributing!
