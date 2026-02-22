# Testing Guide for Shipper

This document describes the comprehensive testing strategy for shipper, a reliability layer around `cargo publish` for Rust workspaces.

## Test Portfolio Overview

Shipper employs a multi-layered testing approach:

| Layer | Tool | Purpose | Location |
|-------|------|---------|----------|
| Unit Tests | `cargo test` | Fast feedback on individual functions | `src/*/tests` modules |
| Property Tests | `proptest` | Invariant checking with random inputs | `src/property_tests.rs` |
| Snapshot Tests | `insta` | Output format stability | `tests/cli_e2e.rs` |
| BDD Tests | Custom | Workflow-driven scenario tests | `tests/bdd_publish.rs` |
| E2E Tests | `assert_cmd` | CLI integration with mocked registry | `tests/cli_e2e.rs` |
| Fuzz Tests | `cargo-fuzz` | Robustness under malformed inputs | `fuzz/fuzz_targets/` |
| Doc Tests | `rustdoc` | Example code validity | Inline in source |

## Running Tests

### Quick Feedback (Local Development)
```bash
# Run all unit and integration tests
cargo test --workspace

# Run with the in-crate modular backends via feature flags
# (auth, git, events, lock, encryption, environment, storage, cargo, registry, process, webhook, types, config, state, store)
cargo test -p shipper --features micro-all

# Run individual backend toggles
cargo test -p shipper --features micro-auth
cargo test -p shipper --features micro-git
cargo test -p shipper --features micro-events
cargo test -p shipper --features micro-lock
cargo test -p shipper --features micro-encrypt
cargo test -p shipper --features micro-environment
cargo test -p shipper --features micro-storage
cargo test -p shipper --features micro-cargo
# Run with the new registry/process/webhook micro backends
cargo test -p shipper --features micro-registry
cargo test -p shipper --features micro-process
cargo test -p shipper --features micro-webhook
cargo test -p shipper --features micro-types
cargo test -p shipper --features micro-config
cargo test -p shipper --features micro-state
cargo test -p shipper --features micro-store

# Run with nextest (faster, better output)
cargo nextest run --workspace

# Run specific test file
cargo test -p shipper-cli --test bdd_publish
```

### CI-Simulated Run
```bash
# Run with CI profile (retries, JUnit output)
cargo nextest run --workspace --profile ci

# With property test cases increased
PROPTEST_CASES=1000 cargo test --workspace
```

### Snapshot Test Review
```bash
# Review pending snapshot updates
cargo insta review

# Accept all pending snapshots
cargo insta accept
```

### Fuzz Testing
```bash
# Install cargo-fuzz (requires nightly)
rustup install nightly
cargo +nightly install cargo-fuzz

# Run fuzz target for 60 seconds
cargo +nightly fuzz run load_state -- -max_total_time=60

# Run with corpus seed
cargo +nightly fuzz run load_state --corpus fuzz/corpus/load_state
```

## Test Categories

### Unit Tests

Unit tests are embedded in each module under `#[cfg(test)] mod tests`. They use:
- `tempfile` for temporary directories
- Mock HTTP servers via `tiny_http`
- Fake cargo/git binaries for hermetic testing

Example:
```rust
#[test]
fn test_version_exists_true_for_200() {
    let server = spawn_registry(vec![200], 1);
    let client = RegistryClient::new(Registry::crates_io()).unwrap();
    let result = client.version_exists("my-crate", "1.0.0").unwrap();
    assert!(result);
}
```

### Property-Based Tests

Property tests verify invariants hold for all inputs:

- **Plan determinism**: Same packages → same plan ID
- **Topo correctness**: Dependencies always before dependents
- **State machine**: Only valid transitions allowed
- **Delay bounds**: Backoff never exceeds configured max

Located in `crates/shipper/src/property_tests.rs`.

### BDD Tests

Behavior-Driven Development tests codify user workflows:

```gherkin
Feature: Resumable publishing

  Scenario: Resume skips cargo publish when state is Uploaded
    Given an existing state file marks "demo@0.1.0" as "Uploaded"
    And the registry returns "published" for "demo@0.1.0"
    When I run "shipper resume"
    Then the exit code is 0
    And cargo publish was not invoked
```

Located in `features/*.feature` and `tests/bdd_publish.rs`.

Micro backend compatibility can be validated from the command line matrix as well:

```bash
# Default behavior (monolithic backends)
cargo test -p shipper-cli --test bdd_publish

# Micro backend behavior (same BDD expectations with feature-flagged microcrates)
cargo test -p shipper-cli --test bdd_publish --features micro-all
cargo test -p shipper-cli --test bdd_publish --features micro-auth
cargo test -p shipper-cli --test bdd_publish --features micro-git
cargo test -p shipper-cli --test bdd_publish --features micro-events
cargo test -p shipper-cli --test bdd_publish --features micro-lock
cargo test -p shipper-cli --test bdd_publish --features micro-encrypt
cargo test -p shipper-cli --test bdd_publish --features micro-environment
cargo test -p shipper-cli --test bdd_publish --features micro-storage
cargo test -p shipper-cli --test bdd_publish --features micro-cargo
cargo test -p shipper-cli --test bdd_publish --features micro-registry
cargo test -p shipper-cli --test bdd_publish --features micro-process
cargo test -p shipper-cli --test bdd_publish --features micro-webhook
cargo test -p shipper-cli --test bdd_publish --features micro-types
cargo test -p shipper-cli --test bdd_publish --features micro-config
cargo test -p shipper-cli --test bdd_publish --features micro-state
cargo test -p shipper-cli --test bdd_publish --features micro-store
cargo test -p shipper-cli --test bdd_publish --features micro-all
``` 

### E2E Tests

End-to-end tests simulate the full CLI workflow:
- Create temporary workspace with multiple crates
- Spawn mock registry server
- Execute real `shipper` binary
- Verify output and state files

### Fuzz Testing

Fuzz targets for security-critical parsing:
- `load_state` - State JSON parsing
- `resolve_token` - Credentials TOML parsing
- `encrypt_decrypt` - Encryption roundtrip
- `retry_strategy` - Delay calculation invariants
- `types_serialization` - JSON serialization

## CI Pipeline

### Main CI (`ci.yml`)
1. **lint** - Format check + clippy
2. **test** - nextest matrix (Linux/Windows/macOS)
3. **msrv** - Minimum Rust version check
4. **security** - `cargo audit`
5. **docs** - Documentation build
6. **coverage** - `cargo llvm-cov`
7. **bdd** - BDD test suite
8. **fuzz-smoke** - Quick fuzz (60s per target)
9. **cross-platform** - Build for all targets

### Scheduled Fuzz (`fuzz.yml`)
- Nightly at 3 AM UTC
- 5 minutes per target
- Crashers uploaded as artifacts

### Release (`release.yml`)
- Triggered by version tags
- Builds binaries for 4 platforms
- Creates GitHub release
- Publishes to crates.io

## Coverage

Coverage reports are generated by `cargo llvm-cov`:

```bash
# Generate LCOV report
cargo llvm-cov --workspace --lcov --output-path lcov.info

# View summary
cargo llvm-cov --workspace
```

Upload to Codecov via CI for trend tracking.

## Writing New Tests

### Adding a Unit Test
1. Find or create the `#[cfg(test)] mod tests` block
2. Follow naming convention: `test_<function>_<scenario>`
3. Use helper functions for common setup

### Adding a BDD Test
1. Add scenario to `features/*.feature`
2. Implement step in `tests/bdd_publish.rs`
3. Run `cargo test -p shipper-cli --test bdd_publish`

### Adding a Fuzz Target
1. Create `fuzz/fuzz_targets/my_target.rs`:
```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Call function with data
    // Assert invariants
});
```

2. Add corpus seeds in `fuzz/corpus/my_target/`
3. Update `fuzz.yml` workflow to include target

## Test Best Practices

1. **Hermetic**: Tests should not depend on external services
2. **Deterministic**: Same inputs → same outputs (use seeds for randomness)
3. **Fast**: Unit tests < 100ms, integration tests < 10s
4. **Isolated**: Each test creates its own temp directory
5. **Descriptive**: Test names should describe the scenario

## Debugging Failed Tests

### Nextest Output
```bash
cargo nextest run --workspace --failure-output immediate
```

### Verbose Logging
```bash
RUST_LOG=debug cargo test --workspace -- --nocapture
```

### Specific Test
```bash
cargo test test_name --exact -- --nocapture
```

### Snapshot Diff
```bash
cargo insta review
