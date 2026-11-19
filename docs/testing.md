# Testing Guide

This document covers the testing infrastructure, conventions, and commands for the
shipper workspace.

---

## Test Types Overview

| Type | Location | Runner | Purpose |
|------|----------|--------|---------|
| **Unit tests** | `#[cfg(test)] mod tests` in source files | `cargo test --workspace` / `cargo nextest run --workspace` | Verify individual functions and modules |
| **Integration tests** | `crates/shipper-cli/tests/e2e_*.rs`, `cli_e2e.rs` | `cargo test -p shipper-cli` | CLI end-to-end against mock registries |
| **BDD tests** | `crates/shipper-cli/tests/bdd_*.rs` + `features/*.feature` | `cargo test -p shipper-cli --test bdd_publish` | Scenario-driven publish/resume/preflight |
| **Snapshot tests** | `crates/shipper-cli/tests/cli_snapshots.rs` and crate-local snapshot tests | `cargo test --workspace` + `cargo insta review` | Pin CLI help text, plans, and structured output |
| **Property tests** | `crates/shipper-core/src/property_tests.rs` and inline modules throughout the workspace | `cargo test --workspace` | Verify invariants hold for arbitrary inputs |
| **Stress tests** | `crates/shipper-core/src/stress_tests.rs` | `cargo test -p shipper-core --lib stress_tests` | Concurrent state access and lock contention |
| **Fuzz tests** | `fuzz/fuzz_targets/*.rs` | `cargo fuzz run <target>` (nightly) | Find panics/crashes from malformed input |
| **Mutation tests** | Selected crates or changed files | `cargo mutants` | Detect undertested code via code mutations |
| **Doc tests** | Inline in source | `cargo test --workspace --doc` | Validate documentation examples |

---

## Running Tests

[Cargo.toml](../Cargo.toml) sets the root `default-members` to `shipper` and
`shipper-cli`. Plain `cargo test` selects those packages' tests. Dependencies
such as `shipper-core` are compiled, but their own test targets are not selected.
Use `--workspace` to include the engine, support crates, and xtask. The `fuzz/`
package is excluded from the product workspace and has its own commands below.

### Workspace tests

```bash
# Standard runner: workspace tests and doc tests, with all features
cargo test --workspace --all-features --locked

# With nextest (used in CI); run doc tests separately
cargo nextest run --workspace --all-features --locked

# CI profile (retries, JUnit output)
cargo nextest run --workspace --all-features --locked --profile ci
```

Nextest runs test binaries; it does not replace the doc-test command below.
These commands retain the runners' normal ignored-test filters.

### Quick default-member check

```bash
cargo test --locked # shipper + shipper-cli only
```

Use this shortcut when those two packages are the intended scope. It does not
establish workspace-wide test coverage.

### Specific crate

```bash
cargo test -p shipper           # install facade + curated re-export tests
cargo test -p shipper-cli       # CLI adapter + integration tests
cargo test -p shipper-core      # engine/library tests
cargo test -p shipper-cargo-failure  # failure classifier
```

### Specific test binary or name

```bash
# A specific integration test binary
cargo test --test cli_e2e -p shipper-cli

# Substring match on test name
cargo test -p shipper-core --lib package_state_roundtrip

# Exact fully qualified test name
cargo test -p shipper-core --lib property_tests::tests::package_state_roundtrip -- --exact
```

### Doc tests

```bash
cargo test --workspace --doc --locked
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

CLI integration snapshots live under `crates/shipper-cli/tests/snapshots/`;
other crates also keep snapshots beside their tests.

### BDD tests

BDD tests implement scenarios from the `features/*.feature` files. They use
`assert_cmd` + `tiny_http` mock registries and run as regular Rust integration
tests.

```bash
cargo test -p shipper-cli --test bdd_publish

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

The routed workspace nextest command includes the BDD integration binaries.
The broader [CI workflow](../.github/workflows/ci.yml) also has a dedicated `bdd`
job for `bdd_publish`, `bdd_resume`, and `e2e_rehearse`. Its explicit manual
hosted route reuses those steps. Earlier RCs ran a feature-flag matrix toggling
`micro-*` backends; both the flags and that matrix were removed during decrating.

### Property tests

Property tests use [proptest](https://proptest-rs.github.io/proptest/) and live
in `crates/shipper-core/src/property_tests.rs` and inline test modules across
the workspace. They verify invariants like serialization roundtrips,
normalization idempotency, and delay bounds.

```bash
# Focus on the core property_tests module
cargo test -p shipper-core --lib property_tests

# Increase case count for that selected module
PROPTEST_CASES=1000 cargo test -p shipper-core --lib property_tests
```

This filter does not select property tests in other modules or crates; the
workspace commands above include them. The
[routed PR workflow](../.github/workflows/em-ci-routed-rust.yml) and the ordinary
nextest step in [broad CI](../.github/workflows/ci.yml) set `PROPTEST_CASES=16`
and `PROPTEST_MAX_SHRINK_ITERS=1000`. Broad CI's separate
`crypto-proptests-heavy` job sets `PROPTEST_CASES=256` only for
`cargo nextest run -p shipper-encrypt --all-features --profile ci`. That job
does not establish a 256-case workspace run. Individual tests can also set
their own proptest configuration.

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
cargo fuzz run load_state fuzz/corpus/load_state
```

Available targets are declared in [fuzz/Cargo.toml](../fuzz/Cargo.toml); use
`cargo fuzz list` for the current names. CI compiles every declared fuzz binary,
while the runtime campaigns select subsets described in
[CI Integration](#ci-integration).

Crashers are stored in `fuzz/artifacts/`, corpus seeds in `fuzz/corpus/`.

### Mutation tests

Mutation testing uses [cargo-mutants](https://mutants.rs/). The
[mutation workflow](../.github/workflows/mutation.yml) has weekly/manual runs
over selected crates and a separate label-gated PR lane over changed files.

```bash
cargo install cargo-mutants

# Run against the same crates as CI
# Note: shipper-plan, shipper-policy, shipper-levels were removed during
# decrating (PRs #54, #56) and are now modules inside shipper-core/.
cargo mutants --no-shuffle \
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
| `proptest` | 1.11 | Property-based / generative testing |
| `serial_test` | 4.x | `#[serial]` attribute for test isolation |
| `temp-env` | 0.3 | Safe scoped environment variable manipulation |
| `serde_yaml` | 0.9 | YAML (de)serialization in BDD tests |

### Nextest profiles

Configured in [`.config/nextest.toml`](../.config/nextest.toml):

| Profile | Retries after failure | Threads | Notes |
|---------|---------|---------|-------|
| `ci` | 2 (exponential, 1s delay) | `num-cpus` | JUnit XML at `target/nextest/ci/junit.xml` |
| `stress` | 10 (fixed, 100ms delay) | 1 | For flaky-test investigation |
| `nightly` | 3 (exponential, 2s delay) | `num-cpus` | Optional profile; select it explicitly |

The `ci` profile sets `fail-fast=false`, so remaining tests continue after a
failure. Retries apply to failed tests; successful tests are not repeated.
`test-threads` controls concurrency, not a timeout. Profile names do not
schedule runs; the current CI commands explicitly select `ci`.

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

The [coverage workflow](../.github/workflows/coverage.yml) selects path-filtered
main pushes, manual dispatch, and same-repository PRs carrying `coverage` or
`full-ci` labels. It generates workspace coverage with `PROPTEST_CASES=16` and
handles Codecov upload separately. Inspect that run and its artifacts before
claiming coverage for a candidate.

---

## CI Integration

The sources below define configured proof scopes. A configured job, queued run,
or historical passing run is not evidence that the current candidate passed.
Retain the source SHA, run ID, selected jobs, terminal results, and relevant
artifacts when making a proof claim. The
[Test Evidence Lanes](ci/test-evidence-lanes.md) document owns the detailed lane
map and manual dispatch procedure.

### PR and broad CI lanes

| Lane and source | Trigger and runner | Configured proof |
|---|---|---|
| [Routed Rust Small](../.github/workflows/em-ci-routed-rust.yml) | Same-repository PRs, main pushes, merge groups, and manual dispatch; selected Linux self-hosted route or Ubuntu hosted fallback | Format, workspace check/Clippy, `cargo nextest run --workspace --locked --all-targets --all-features --profile ci`, workspace doc tests, `cargo package --workspace --locked --exclude xtask`, and CLI help smoke. The normalized result is `Shipper Rust Small Result`. |
| [Broad CI](../.github/workflows/ci.yml) | Main pushes, manual dispatch, and Sunday 03:00 UTC; Linux self-hosted by default | Workspace nextest and doc tests, lint, policy, MSRV, security, docs, install smoke, BDD/recovery scenarios, and the target check. Ordinary nextest sets `PROPTEST_CASES=16`. |
| Broad CI manual hosted route | Explicit `workflow_dispatch` with `runner_route=github-hosted` in `shipper-swarm`; Ubuntu 24.04 | Reuses all twelve broad-CI job step sets, including heavy crypto, fuzz smoke, and release-profile compilation. It is separate from the routed PR fallback. |
| Broad CI heavy crypto | Main pushes, manual dispatch, and the weekly schedule | `shipper-encrypt` only, with `PROPTEST_CASES=256`. |

The hosted broad route was added by merged
[PR #373](https://github.com/EffortlessMetrics/shipper-swarm/pull/373).
[Issue #370](https://github.com/EffortlessMetrics/shipper-swarm/issues/370)
tracks self-hosted execution availability. Neither route configuration nor a
past successful dispatch proves that a new candidate has completed broad CI.

### Fuzz and mutation scope

| Source | Trigger | Configured proof |
|---|---|---|
| `fuzz-smoke` in [broad CI](../.github/workflows/ci.yml) | Main pushes and manual dispatch; skipped by the weekly schedule | Five targets (`load_state`, `resolve_token`, `schema_version`, `release_levels`, `redact_output`) with a 60-second budget each. `ci.yml` has no PR trigger. |
| `check-targets` in [fuzz.yml](../.github/workflows/fuzz.yml) | Path-filtered PRs/main pushes, nightly schedule, and manual dispatch | Nightly Rust `cargo check --manifest-path fuzz/Cargo.toml --bins` compiles every declared fuzz binary. Compilation does not execute fuzz campaigns. |
| `fuzz` in [fuzz.yml](../.github/workflows/fuzz.yml) | Nightly at 03:00 UTC and manual dispatch only | Default 300-second budget for each of six targets: `load_state`, `resolve_token`, `encrypt_decrypt`, `retry_strategy`, `types_serialization`, and `redact_output`. Manual `targets=all` selects this six-target set; explicit target names select a different set. |
| [mutation.yml](../.github/workflows/mutation.yml) | Sunday 04:00 UTC or manual dispatch; separate same-repository PR lane gated by `mutation`/`full-ci` labels | Weekly/manual runs target `shipper-duration`, `shipper-types`, and `shipper-config`. The PR lane scopes mutation to changed files. |

### Platform and release-build limits

Workspace test and doc-test execution in both ordinary CI workflows is on
Linux. The `cross-platform` job in `ci.yml` currently runs
`cargo check --workspace --target x86_64-unknown-linux-gnu`; it does not execute
tests or supply a Windows/macOS compile matrix. A cross-target compile check,
when configured, would still be distinct from runtime tests on that platform.

Broad CI's `release-build` runs `cargo build --release` for the default members
on main pushes and manual dispatch, and uploads the Linux CLI artifact. It is
skipped on the weekly schedule. The release authority's separate
[`release.yml`](../.github/workflows/release.yml) configures native Linux,
macOS (x86_64 and aarch64), and Windows binary builds plus packaged-executable
smoke proof. Those jobs do not run a workspace test matrix on all those systems.
Use the [support tiers](status/SUPPORT_TIERS.md) and exact-candidate release
evidence for platform claims; these workflow definitions do not promote them.

---

## Debugging Failed Tests

```bash
# Immediate failure output with nextest
cargo nextest run --workspace --failure-output immediate

# Verbose output for a single test
cargo test -p shipper-core --lib property_tests::tests::package_state_roundtrip -- --exact --nocapture

# Review snapshot diffs
cargo insta review
```
