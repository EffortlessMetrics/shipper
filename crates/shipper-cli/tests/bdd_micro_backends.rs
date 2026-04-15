//! BDD (Behavior-Driven Development) tests for preflight stability.
//!
//! These tests verify that shipper's preflight behavior remains stable.
//! Historically these tests toggled `micro-*` feature flags that swapped
//! in-tree modules for external microcrates; those flags and the dual
//! implementation were removed as part of the decrating effort, leaving
//! a single canonical code path that these tests still exercise.

use std::fs;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

/// Create a workspace with a dependency chain (core → utils → app).
fn create_workspace_with_dependency_chain(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "utils", "app"]
resolver = "2"
"#,
    );

    // Core crate (no dependencies)
    write_file(
        &root.join("core/Cargo.toml"),
        r#"
[package]
name = "core"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("core/src/lib.rs"), "pub fn core() {}\n");

    // Utils crate (depends on core)
    write_file(
        &root.join("utils/Cargo.toml"),
        r#"
[package]
name = "utils"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core" }
"#,
    );
    write_file(&root.join("utils/src/lib.rs"), "pub fn utils() {}\n");

    // App crate (depends on utils and core)
    write_file(
        &root.join("app/Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core" }
utils = { path = "../utils" }
"#,
    );
    write_file(&root.join("app/src/lib.rs"), "pub fn app() {}\n");
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

struct TestRegistry {
    base_url: String,
    handle: thread::JoinHandle<()>,
}

impl TestRegistry {
    fn join(self) {
        self.handle.join().expect("join server");
    }
}

/// Spawn a mock registry that returns 404 for all requests (simulating "not found").
fn spawn_not_found_registry(expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for _ in 0..expected_requests {
            let req = match server.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(Some(r)) => r,
                _ => break,
            };
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(404))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json").expect("header"),
                );
            req.respond(resp).expect("respond");
        }
    });
    TestRegistry { base_url, handle }
}

// ============================================================================
// Feature: Preflight stability
// ============================================================================

mod preflight_stability {
    use super::*;

    // Scenario: Preflight behavior stays stable
    //   Given a workspace with a dependency chain
    //   And no registry token is configured
    //   And the registry returns "not found" for all crates
    //   When I run "shipper preflight" with "--policy fast" and "--allow-dirty"
    //   Then the preflight report shows token not detected
    //   And the exit code is 0
    #[test]
    fn given_dependency_chain_and_no_token_when_preflight_then_stable_output() {
        // Given: A workspace with a dependency chain
        let td = tempdir().expect("tempdir");
        create_workspace_with_dependency_chain(td.path());

        // And: No registry token is configured
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // And: The registry returns "not found" for all crates
        // 3 crates x (version check + new crate check) = 6 requests
        let registry = spawn_not_found_registry(6);

        // When: I run "shipper preflight" with "--policy fast" and "--allow-dirty"
        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then: The preflight report shows token not detected
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false"),
            "Expected token-not-detected indicator in output, got:\n{stdout}"
        );

        // And: The exit code is 0 (verified by .success() above)

        registry.join();
    }
}
