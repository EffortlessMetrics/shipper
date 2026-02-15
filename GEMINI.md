# GEMINI.md - Project Context: Shipper

`shipper` is a **publishing reliability layer** for Rust workspaces. It is designed to make the process of publishing multiple crates safer, deterministic, and resumable, addressing common real-world failures like partial publishes, CI cancellations, and registry backpressure.

## Project Overview

- **Core Purpose:** Enhances `cargo publish` by adding a reliability layer that handles planning, preflight checks, retries, and state persistence.
- **Main Technologies:** Rust (Edition 2024), `clap` (CLI), `anyhow` (Error Handling), `serde` (Serialization), `tokio` (Async - though much of the current logic is sync with thread sleeps), `chrono` (Time).
- **Architecture:**
    - **`crates/shipper` (Library):** The engine of the project. Contains logic for:
        - **Planning:** Building a dependency-aware publish order.
        - **Preflight:** Verifying git state, crate ownership, and registry reachability.
        - **Engine:** Executing the publish plan with backoff and retries.
        - **Registry:** Interacting with Cargo registries (crates.io by default) via their web APIs.
        - **State:** Persisting execution progress to disk for resumability.
    - **`crates/shipper-cli` (Binary):** A CLI wrapper around the library logic, providing commands for the end-user.

## Building and Running

- **Build:** `cargo build`
- **Install CLI:** `cargo install --path crates/shipper-cli --locked`
- **Test:** `cargo test` (Note: some tests use `serial_test` as they modify environment variables or global state).
- **Fuzzing:** Located in `fuzz/` directory; can be run with `cargo-fuzz`.

## Key Commands (via `shipper-cli`)

- `shipper plan`: Builds and displays the deterministic publish order.
- `shipper preflight`: Runs all safety checks (git cleanliness, ownership, version existence) without publishing.
- `shipper publish`: Executes the plan, writing state to `.shipper/state.json` and a receipt to `.shipper/receipt.json`.
- `shipper resume`: Continues an interrupted publish run using the existing state file.
- `shipper status`: Compares local workspace versions against the registry.
- `shipper doctor`: Diagnostics for the environment, authentication (CARGO_REGISTRY_TOKEN), and tool versions.

## Development Conventions

- **Safety:** The project enforces `#[forbid(unsafe_code)]` in the workspace.
- **Error Handling:** Uses `anyhow::Result` for flexible error reporting across the library and CLI.
- **Reporting:** Uses a `Reporter` trait to abstract logging/output, allowing the CLI to provide formatted eprints while keeping the library agnostic.
- **State Management:** Execution state is persisted atomically as JSON. The `plan_id` is used to ensure that resumes match the intended plan.
- **Testing Pattern:** 
    - Extensive use of `tempfile` for filesystem isolation.
    - Registry interactions are mocked in tests using a local `tiny_http` server.
    - `insta` is used for snapshot testing in some modules.
- **Registry Integration:** Uses `CARGO_REGISTRY_TOKEN` and `CARGO_HOME/credentials.toml` for authentication, mimicking Cargo's own behavior.

## Project Structure

- `crates/shipper/src/`:
    - `lib.rs`: Module declarations.
    - `engine.rs`: The main execution loop for publishing.
    - `plan.rs`: Workspace analysis and dependency sorting.
    - `registry.rs`: HTTP client for registry APIs.
    - `auth.rs`: Token resolution logic.
    - `state.rs`: Persistence logic for `state.json` and `receipt.json`.
    - `types.rs`: Shared data structures (Plans, States, Receipts).
- `crates/shipper-cli/src/main.rs`: CLI entry point and argument parsing.
- `templates/`: Example CI/CD configurations (GitHub/GitLab).
- `fuzz/`: Fuzzing targets for robust state loading and token resolution.
