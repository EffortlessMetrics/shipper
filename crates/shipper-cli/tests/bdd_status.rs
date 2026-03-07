//! BDD (Behavior-Driven Development) tests for the shipper status command.
//!
//! These tests describe the expected behavior of `shipper status` in various
//! scenarios using Given-When-Then style documentation.

use std::fs;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
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

/// Spawn a mock registry that responds with the given HTTP status codes.
/// `statuses` is cycled for each request; `expected_requests` is how many
/// requests the mock will serve before shutting down.
fn spawn_registry(statuses: Vec<u16>, expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for idx in 0..expected_requests {
            let req = match server.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Some(r)) => r,
                _ => break,
            };
            let status = statuses
                .get(idx)
                .copied()
                .or_else(|| statuses.last().copied())
                .unwrap_or(404);
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(status))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json").expect("header"),
                );
            req.respond(resp).expect("respond");
        }
    });
    TestRegistry { base_url, handle }
}

/// Create a simple workspace with a single crate.
fn create_simple_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["alpha"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("alpha/Cargo.toml"),
        r#"
[package]
name = "alpha"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("alpha/src/lib.rs"), "pub fn alpha() {}\n");
}

/// Create a workspace with multiple crates that have inter-dependencies.
fn create_multi_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core-lib", "mid-lib", "top-app"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("core-lib/Cargo.toml"),
        r#"
[package]
name = "core-lib"
version = "0.2.0"
edition = "2021"
"#,
    );
    write_file(&root.join("core-lib/src/lib.rs"), "pub fn core() {}\n");

    write_file(
        &root.join("mid-lib/Cargo.toml"),
        r#"
[package]
name = "mid-lib"
version = "0.3.0"
edition = "2021"

[dependencies]
core-lib = { path = "../core-lib" }
"#,
    );
    write_file(
        &root.join("mid-lib/src/lib.rs"),
        "pub fn mid() { core_lib::core(); }\n",
    );

    write_file(
        &root.join("top-app/Cargo.toml"),
        r#"
[package]
name = "top-app"
version = "0.4.0"
edition = "2021"

[dependencies]
mid-lib = { path = "../mid-lib" }
"#,
    );
    write_file(
        &root.join("top-app/src/lib.rs"),
        "pub fn top() { mid_lib::mid(); }\n",
    );
}

// ============================================================================
// Feature: Status Command — Version Reporting
// ============================================================================

mod version_reporting {
    use super::*;

    // Scenario 1: All package versions are shown
    #[test]
    fn given_a_workspace_when_running_status_then_all_package_versions_are_shown() {
        // Given: A workspace with three crates at different versions
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        // Registry returns 404 for all → all "missing"
        let registry = spawn_registry(vec![404], 3);

        // When: We run shipper status
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("status")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).expect("utf8");

        // Then: All package versions are shown in the output
        assert!(
            stdout.contains("core-lib@0.2.0"),
            "expected core-lib@0.2.0 in output, got: {stdout}"
        );
        assert!(
            stdout.contains("mid-lib@0.3.0"),
            "expected mid-lib@0.3.0 in output, got: {stdout}"
        );
        assert!(
            stdout.contains("top-app@0.4.0"),
            "expected top-app@0.4.0 in output, got: {stdout}"
        );

        registry.join();
    }

    // Scenario 1b: Single crate workspace shows its version
    #[test]
    fn given_a_single_crate_workspace_when_running_status_then_version_is_shown() {
        // Given: A workspace with one crate
        let td = tempdir().expect("tempdir");
        create_simple_workspace(td.path());

        let registry = spawn_registry(vec![404], 1);

        // When: We run shipper status
        // Then: The single crate version is shown
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("status")
            .assert()
            .success()
            .stdout(contains("alpha@0.1.0"));

        registry.join();
    }
}

// ============================================================================
// Feature: Status Command — Published Status Detection
// ============================================================================

mod published_status_detection {
    use super::*;

    // Scenario 2: Published crates are shown as published
    #[test]
    fn given_workspace_with_published_crates_when_running_status_then_published_status_is_shown() {
        // Given: A workspace with a single crate
        let td = tempdir().expect("tempdir");
        create_simple_workspace(td.path());

        // And: The registry reports the version exists (200)
        let registry = spawn_registry(vec![200], 1);

        // When: We run shipper status
        // Then: The crate is shown as "published"
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("status")
            .assert()
            .success()
            .stdout(contains("alpha@0.1.0: published"));

        registry.join();
    }

    // Scenario 2b: Unpublished crates are shown as missing
    #[test]
    fn given_workspace_with_unpublished_crates_when_running_status_then_missing_status_is_shown() {
        // Given: A workspace with a single crate
        let td = tempdir().expect("tempdir");
        create_simple_workspace(td.path());

        // And: The registry reports version not found (404)
        let registry = spawn_registry(vec![404], 1);

        // When: We run shipper status
        // Then: The crate is shown as "missing"
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("status")
            .assert()
            .success()
            .stdout(contains("alpha@0.1.0: missing"));

        registry.join();
    }

    // Scenario 2c: Mixed published/missing workspace
    #[test]
    fn given_workspace_with_mixed_versions_when_running_status_then_each_status_is_correct() {
        // Given: A multi-crate workspace
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        // And: First crate is published (200), remaining are missing (404)
        let registry = spawn_registry(vec![200, 404, 404], 3);

        // When: We run shipper status
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("status")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).expect("utf8");

        // Then: At least one crate is shown as published and at least one as missing
        assert!(
            stdout.contains("published"),
            "expected at least one published crate"
        );
        assert!(
            stdout.contains("missing"),
            "expected at least one missing crate"
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Status Command — Package Filtering
// ============================================================================

mod package_filtering {
    use super::*;

    // Scenario 3: --package filter shows only the specified package
    #[test]
    fn given_workspace_with_mixed_versions_when_running_status_with_package_then_only_that_package_is_shown()
     {
        // Given: A multi-crate workspace
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        // Only one crate should be queried
        let registry = spawn_registry(vec![404], 1);

        // When: We run shipper status --package core-lib
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--package")
            .arg("core-lib")
            .arg("status")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).expect("utf8");

        // Then: Only core-lib is shown
        assert!(
            stdout.contains("core-lib@0.2.0"),
            "expected core-lib@0.2.0 in output"
        );
        // And: Other packages are NOT shown
        assert!(
            !stdout.contains("mid-lib"),
            "mid-lib should not appear in filtered output"
        );
        assert!(
            !stdout.contains("top-app"),
            "top-app should not appear in filtered output"
        );

        registry.join();
    }

    // Scenario 3b: --package with multiple packages
    #[test]
    fn given_workspace_when_running_status_with_multiple_packages_then_only_those_are_shown() {
        // Given: A multi-crate workspace
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        // Two crates queried
        let registry = spawn_registry(vec![404], 2);

        // When: We run shipper status --package core-lib --package mid-lib
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--package")
            .arg("core-lib")
            .arg("--package")
            .arg("mid-lib")
            .arg("status")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).expect("utf8");

        // Then: Both filtered packages are shown
        assert!(stdout.contains("core-lib@0.2.0"), "expected core-lib");
        assert!(stdout.contains("mid-lib@0.3.0"), "expected mid-lib");
        // And: top-app is NOT shown
        assert!(
            !stdout.contains("top-app"),
            "top-app should not appear in filtered output"
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Status Command — Error Handling (No Workspace)
// ============================================================================

mod no_workspace_error {
    use super::*;

    // Scenario 4: No workspace → clear error message
    #[test]
    fn given_no_workspace_when_running_status_then_error_message_is_clear() {
        // Given: A directory with no Cargo.toml (not a workspace)
        let td = tempdir().expect("tempdir");
        write_file(&td.path().join("README.md"), "not a workspace");

        // When: We run shipper status pointing at the non-workspace directory
        // Then: The command fails
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("status")
            .assert()
            .failure();
    }

    // Scenario 4b: Empty directory → failure with no panic
    #[test]
    fn given_empty_directory_when_running_status_then_command_fails_gracefully() {
        // Given: An empty temp directory
        let td = tempdir().expect("tempdir");

        // When: We run shipper status
        // Then: The command fails (no panic, exits with non-zero)
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("status")
            .assert()
            .failure();
    }
}

// ============================================================================
// Feature: Status Command — Connectivity Failures
// ============================================================================

mod connectivity_failures {
    use super::*;

    // Scenario 5: Registry unreachable → error is handled gracefully
    #[test]
    fn given_workspace_with_no_connectivity_when_running_status_then_error_is_handled_gracefully() {
        // Given: A workspace with a single crate
        let td = tempdir().expect("tempdir");
        create_simple_workspace(td.path());

        // And: The registry endpoint points to a port that is not listening
        //      (bind then immediately drop to get a guaranteed-closed port)
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let dead_port = listener.local_addr().expect("local_addr").port();
        drop(listener);

        let dead_url = format!("http://127.0.0.1:{dead_port}");

        // When: We run shipper status against the unreachable registry
        // Then: The command fails without panicking (exit code != 0)
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&dead_url)
            .arg("status")
            .assert()
            .failure();
    }
}
