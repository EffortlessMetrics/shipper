# Testing Guide

This document covers the testing infrastructure, conventions, and commands for the
shipper workspace.

---

## Test Types Overview

| Type | Location | Runner | Purpose |
|------|----------|--------|---------|
| **Unit tests** | `#[cfg(test)] mod tests` in source files | `cargo test` / `cargo nextest` | Verify individual functions and modules |
| **Integration tests** | `crates/shipper-cli/tests/e2e_*.rs`, `cli_e2e.rs` | `cargo test -p shipper-cli` | CLI end-to-end against mock registries |
| **BDD tests** | `crates/shipper-cli/tests/bdd_*.rs` + `features/*.feature` | `cargo test -p shipper-cli --test bdd_publish` | Scenario-driven publish/resume/preflight |
| **Snapshot tests** | `crates/shipper-cli/tests/cli_snapshots.rs` | `cargo test` + `cargo insta review` | Pin CLI help text and plan output |
| **Property tests** | `crates/shipper/src/property_tests.rs` | `cargo test -p shipper` | Verify invariants hold for arbitrary inputs |
| **Stress tests** | `crates/shipper/src/stress_tests.rs` | `cargo test -p shipper` | Concurrent state access and lock contention |
| **Fuzz tests** | `fuzz/fuzz_targets/*.rs` | `cargo fuzz run <target>` (nightly) | Find panics/crashes from malformed input |
| **Mutation tests** | CI only | `cargo mutants` | Detect undertested code via code mutations |
| **Doc tests** | Inline in source | `cargo test --doc` | Validate documentation examples |

---

## Running Tests

### All tests (workspace)

```bash
# Standard test runner
cargo test --workspace --all-features

# With nextest (recommended — used in CI)
cargo nextest run --workspace --all-features

# CI profile (retries, JUnit output)
cargo nextest run --workspace --all-features --profile ci
```

### Specific crate

```bash
cargo test -p shipper           # core library
cargo test -p shipper-cli       # CLI + integration tests
cargo test -p shipper-cargo-failure  # example microcrate (failure classifier)
```

### Specific test binary or name

```bash
# A specific integration test binary
cargo test --test cli_e2e -p shipper-cli

# Substring match on test name
cargo test -p shipper plan_determinism

# Exact test name
cargo test -p shipper plan_determinism -- --exact
```

### Doc tests

```bash
cargo test --workspace --doc
```

### Snapshot tests

Snapshots are managed by [insta](https://insta.rs). In CI, `INSTA_UPDATE=no`
prevents auto-updating; new/changed snapshots fail the build.

```bash
# Run snapshot tests
cargo test --test cli_snapshots -p shipper-cli

# Review pending snapshot changes interactively
cargo insta review

# Accept all pending snapshots
cargo insta accept
```

Snapshot files live under `crates/shipper-cli/tests/snapshots/`.

### BDD tests

BDD tests implement scenarios from the `features/*.feature` files. They use
`assert_cmd` + `tiny_http` mock registries and run as regular Rust integration
tests.

```bash
# Default (all micro-backends enabled)
cargo test -p shipper-cli --test bdd_publish

# With a specific micro-backend feature set
cargo test -p shipper-cli --test bdd_publish --features micro-git

# Other BDD suites
cargo test -p shipper-cli --test bdd_preflight
cargo test -p shipper-cli --test bdd_resume
cargo test -p shipper-cli --test bdd_parallel
cargo test -p shipper-cli --test bdd_micro_backends
```

Feature files in `features/`:

- `publish_resume.feature` — publish + resume lifecycle
- `preflight_checks.feature` — preflight verification scenarios
- `parallel_levels.feature` — parallel level grouping
- `micro_backend_feature_flags.feature` — micro-backend stability

In CI, BDD tests run across a matrix of all micro-backend feature combinations
(see `.github/workflows/ci.yml`, `bdd` job).

### Property tests

Property tests use [proptest](https://proptest-rs.github.io/proptest/) and live
in `crates/shipper/src/property_tests.rs`. They verify invariants like
serialization roundtrips, normalization idempotency, and delay bounds.

```bash
cargo test -p shipper property_tests

# Increase case count for deeper coverage
PROPTEST_CASES=1000 cargo test -p shipper property_tests
```

CI runs with `PROPTEST_CASES=256`.

### Fuzz tests

Fuzz tests require **nightly Rust** and `cargo-fuzz`.

```bash
# Install prerequisites
rustup install nightly
cargo install cargo-fuzz

# List available targets
cargo fuzz list

# Run a target (60-second smoke test)
cargo fuzz run load_state -- -max_total_time=60

# Run with a corpus directory
cargo fuzz run load_state --corpus fuzz/corpus/load_state
```

Available fuzz targets:

| Target | Tests |
|--------|-------|
| `load_state` | State file deserialization |
| `state_load` | State loading (alternate path) |
| `receipt_load` | Receipt loading |
| `resolve_token` | Token resolution paths |
| `schema_version` | Schema version parsing |
| `policy_effects` | Policy evaluation |
| `release_levels` | Level computation |
| `redact_output` | Output sanitization |
| `duration_codec` | Duration encode/decode |
| `config_parse` | `.shipper.toml` parsing |
| `config_runtime_adapter` | Config runtime adapter |
| `cargo_failure_classifier` | Failure classification |
| `engine_parallel_chunks` | Parallel chunking |
| `execution_core` | Execution core logic |
| `encrypt_decrypt` | Encryption roundtrip |
| `retry_strategy` | Retry strategy evaluation |
| `types_serialization` | Types serde roundtrip |
| `sparse_index` | Sparse index parsing |
| `webhook_payload` | Webhook payload handling |
| `plan_builder` | Plan construction |
| `git_context` | Git context parsing |

Crashers are stored in `fuzz/artifacts/`, corpus seeds in `fuzz/corpus/`.

### Mutation tests

Mutation testing uses [cargo-mutants](https://mutants.rs/) and runs weekly in CI.

```bash
cargo install cargo-mutants

# Run against the same crates as CI
# Note: shipper-plan, shipper-policy, shipper-levels were removed during
# decrating (PRs #54, #56) and are now modules inside shipper/.
cargo mutants --no-shuffle \
  -p shipper-schema \
  -p shipper-duration \
  -p shipper-types \
  -p shipper-config \
  -- --all-features
```

Results are written to `mutants.out/`.

---

## Test Infrastructure

### Libraries

| Crate | Version | Purpose |
|-------|---------|---------|
| `assert_cmd` | 2.x | Run CLI binary and assert exit code / output |
| `predicates` | 3.x | Fluent assertions for `assert_cmd` output |
| `tempfile` | 3.x | Temporary directories for filesystem isolation |
| `tiny_http` | 0.12 | In-process HTTP server for mock registries |
| `insta` | 1.x (yaml feature) | Snapshot testing for CLI output |
| `proptest` | 1.10 | Property-based / generative testing |
| `serial_test` | 3.x | `#[serial]` attribute for test isolation |
| `temp-env` | 0.3 | Safe scoped environment variable manipulation |
| `serde_yaml` | 0.9 | YAML (de)serialization in BDD tests |

### Nextest profiles

Configured in `.config/nextest.toml`:

| Profile | Retries | Threads | Notes |
|---------|---------|---------|-------|
| `ci` | 2 (exponential, 1s delay) | `num-cpus` | JUnit XML at `target/nextest/ci/junit.xml` |
| `stress` | 10 (fixed, 100ms delay) | 1 | For flaky-test investigation |
| `nightly` | 3 (exponential, 2s delay) | `num-cpus` | Extended scheduled testing |

```bash
cargo nextest run --profile ci --workspace --all-features
```

---

## Writing New Tests

### Environment variables

The workspace uses **Rust edition 2024** and `#[forbid(unsafe_code)]`.
`std::env::set_var` is `unsafe` in edition 2024, so **always** use `temp_env`:

```rust
use temp_env::with_vars;

#[test]
fn token_from_env() {
    with_vars(
        [("CARGO_REGISTRY_TOKEN", Some("secret"))],
        || {
            // test logic that reads the env var
        },
    );
}
```

**Never** call `std::env::set_var` or `std::env::remove_var` directly.

### Serial test isolation

Tests that touch shared global state (environment, filesystem singletons) must
use the `#[serial]` attribute from `serial_test`:

```rust
use serial_test::serial;

#[test]
#[serial]
fn test_that_modifies_global_state() {
    // ...
}
```

### Filesystem tests

Always create temporary directories with `tempfile::tempdir()` so cleanup
happens automatically:

```rust
use tempfile::tempdir;

#[test]
fn state_roundtrip() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("state.json");
    // write and read state_path...
}
```

### Mock registry pattern

BDD and E2E tests spin up a `tiny_http::Server` to mock the crates.io API.
The typical pattern:

```rust
use tiny_http::{Server, Response, StatusCode, Header};
use std::thread;

let server = Server::http("127.0.0.1:0").unwrap();
let port = server.server_addr().to_ip().unwrap().port();

// Spawn responder thread
let handle = thread::spawn(move || {
    while let Ok(req) = server.recv() {
        let path = req.url().to_string();
        let resp = if path.contains("/api/v1/crates/") {
            Response::from_string(r#"{"errors":[{"detail":"Not Found"}]}"#)
                .with_status_code(StatusCode(404))
        } else {
            Response::from_string("ok")
        };
        req.respond(resp).ok();
    }
});

// Point the CLI at the mock registry
// ... run assert_cmd with --registry-url http://127.0.0.1:{port}
```

### CLI integration tests

Use `assert_cmd::Command` to test the compiled CLI binary:

```rust
use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn plan_shows_packages() {
    Command::cargo_bin("shipper")
        .unwrap()
        .args(["plan", "--manifest-path", "fixtures/Cargo.toml"])
        .assert()
        .success()
        .stdout(contains("demo@0.1.0"));
}
```

### Snapshot tests

When adding new CLI output that should be pinned, use `insta::assert_snapshot!`:

```rust
use insta::assert_snapshot;

#[test]
fn new_command_help() {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
        .args(["new-cmd", "--help"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("new_cmd_help", redact_version(&stdout));
}
```

Run `cargo insta review` after the test to accept the initial snapshot.

### Adding a fuzz target

1. Create `fuzz/fuzz_targets/my_target.rs`:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Call function under test with arbitrary data
    // Assert invariants (no panics, roundtrip consistency, etc.)
});
```

2. Register the binary in `fuzz/Cargo.toml`:

```toml
[[bin]]
name = "my_target"
path = "fuzz_targets/my_target.rs"
test = false
doc = false
bench = false
```

3. Add corpus seeds in `fuzz/corpus/my_target/`.
4. Add the target to `.github/workflows/fuzz.yml` matrix.

---

## Coverage

Coverage uses [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov)
with the `llvm-tools-preview` component.

```bash
# Install
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov

# Generate LCOV report
cargo llvm-cov --workspace --all-features --lcov --output-path lcov.info

# Generate HTML report (open in browser)
cargo llvm-cov --workspace --all-features --html
open target/llvm-cov/html/index.html

# Quick summary to terminal
cargo llvm-cov --workspace --all-features
```

CI uploads `lcov.info` to Codecov on every push to `main`.

---

## CI Integration

CI is defined in `.github/workflows/ci.yml` and runs on every push to `main`
and every pull request. The pipeline includes:

| Job | Runs on | What it does |
|-----|---------|--------------|
| **Lint** | ubuntu | `cargo fmt --check` + `cargo clippy -D warnings` |
| **Tests** | ubuntu, windows, macos | `cargo nextest run` with `INSTA_UPDATE=no`, `PROPTEST_CASES=256` |
| **Doc tests** | ubuntu, windows, macos | `cargo test --workspace --doc` |
| **BDD** | ubuntu | BDD suites across all micro-backend feature combinations |
| **MSRV** | ubuntu | `cargo check` with Rust 1.92 |
| **Security** | ubuntu | `cargo audit` |
| **Docs** | ubuntu | `cargo doc` with `-Dwarnings` |
| **Coverage** | ubuntu | `cargo llvm-cov` → Codecov |
| **Fuzz smoke** | ubuntu (PRs) | Each target for 60 seconds |
| **Cross-platform** | matrix | Build for x86_64/aarch64 on Linux, Windows, macOS |
| **Release build** | ubuntu | `cargo build --release` |

Additional scheduled workflows:

| Workflow | Schedule | What it does |
|----------|----------|--------------|
| **Fuzz** (`.github/workflows/fuzz.yml`) | Nightly 3 AM UTC | Extended fuzzing, 5 min per target |
| **Mutation** (`.github/workflows/mutation.yml`) | Weekly Sunday 4 AM UTC | `cargo mutants` on core crates |

---

## Debugging Failed Tests

```bash
# Immediate failure output with nextest
cargo nextest run --workspace --failure-output immediate

# Verbose output for a single test
cargo test -p shipper test_name -- --exact --nocapture

# Review snapshot diffs
cargo insta review
```
