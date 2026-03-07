//! BDD (Behavior-Driven Development) tests for the shipper preflight command.
//!
//! These tests describe the expected behavior of `shipper preflight` in various
//! scenarios using Given-When-Then style documentation, covering all scenarios
//! from features/preflight_checks.feature.

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

/// Background: a workspace with crates "core" and "app" where "app" depends on "core"
fn create_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "app"]
resolver = "2"
"#,
    );

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

    write_file(
        &root.join("app/Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core" }
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

fn spawn_registry(statuses: Vec<u16>, expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for idx in 0..expected_requests {
            let req = match server.recv_timeout(std::time::Duration::from_secs(30)) {
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

// ============================================================================
// Feature: Preflight Verification
// ============================================================================

mod preflight_passes_for_new_crates {
    use super::*;

    // Scenario: Preflight passes for new crates with token
    //   Given a workspace with crates "core" and "app" where "app" depends on "core"
    //   And a valid registry token is configured
    //   And the registry returns "not found" for all crates
    //   When I run "shipper preflight"
    //   Then the exit code is 0
    //   And all packages are marked as new crates
    #[test]
    fn given_token_and_new_crates_when_preflight_then_passes_and_marks_new() {
        // Given: workspace with core→app, token configured, registry 404 for all
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // 2 crates × 2 requests each (version_exists + check_new_crate) = 4
        let registry = spawn_registry(vec![404, 404, 404, 404], 4);

        // When: running preflight with token, --allow-dirty, --skip-ownership-check
        let out = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--skip-ownership-check")
            .arg("--policy")
            .arg("fast")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env("CARGO_REGISTRY_TOKEN", "fake-token-for-test")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then: exit code is 0 and all packages are marked as new crates
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("New crates: 2"),
            "Expected 'New crates: 2' in output, got:\n{stdout}"
        );
        assert!(
            stdout.contains("Token Detected: ✓") || stdout.contains("\"token_detected\":true"),
            "Expected token detected in output, got:\n{stdout}"
        );
        assert!(
            stdout.contains("Total packages: 2"),
            "Expected 'Total packages: 2' in output, got:\n{stdout}"
        );

        registry.join();
    }
}

mod preflight_policy_from_config {
    use super::*;

    // Scenario: Preflight uses policy from .shipper.toml
    //   Given a workspace with crates "core" and "app" where "app" depends on "core"
    //   And a file named ".shipper.toml" with policy set to "fast"
    //   When I run "shipper preflight" without passing --policy
    //   Then the exit code is 0
    //   And the preflight report shows token not detected
    #[test]
    fn given_shipper_toml_policy_when_preflight_without_flag_then_uses_config_policy() {
        // Given: workspace with .shipper.toml setting policy to "fast"
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        write_file(
            &td.path().join(".shipper.toml"),
            r#"
[policy]
mode = "fast"
"#,
        );

        // 2 crates × 2 requests (fast policy: version_exists + check_new_crate only) = 4
        let registry = spawn_registry(vec![404, 404, 404, 404], 4);

        // When: running preflight WITHOUT --policy flag (should use .shipper.toml)
        let out = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then: token is not detected (fast policy from config was used)
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false"),
            "Expected token not detected in output, got:\n{stdout}"
        );

        registry.join();
    }
}

mod preflight_detects_already_published {
    use super::*;

    // Scenario: Preflight detects already published versions
    //   Given a valid registry token is configured
    //   And the registry returns "published" for "core@0.1.0"
    //   And the registry returns "not found" for "app@0.1.0"
    //   When I run "shipper preflight"
    //   Then the preflight report shows "core@0.1.0" as already published
    //   And the preflight report shows "app@0.1.0" as not published
    #[test]
    fn given_mixed_published_when_preflight_then_detects_already_published() {
        // Given: core is published (200), app is not (404)
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // Request sequence (plan order: core first, then app):
        //   1. core version_exists → 200 (already published)
        //   2. core check_new_crate → 200 (exists, not new)
        //   3. app version_exists → 404 (not published)
        //   4. app check_new_crate → 404 (new crate)
        let registry = spawn_registry(vec![200, 200, 404, 404], 4);

        // When: running preflight with --policy fast to skip dry-run
        let out = shipper_cmd()
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

        // Then: report shows 1 already published and 1 new crate
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("Already published: 1"),
            "Expected 'Already published: 1' in output, got:\n{stdout}"
        );
        assert!(
            stdout.contains("New crates: 1"),
            "Expected 'New crates: 1' in output, got:\n{stdout}"
        );

        registry.join();
    }
}

mod preflight_warns_on_missing_token {
    use super::*;

    // Scenario: Preflight warns on missing token
    //   Given no registry token is configured
    //   And the registry returns "not found" for all crates
    //   When I run "shipper preflight" with "--policy fast"
    //   Then the preflight report shows token not detected
    //   And the exit code is 0
    #[test]
    fn given_no_token_when_preflight_fast_then_reports_token_not_detected() {
        // Given: no token configured, registry 404 for all
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // 2 crates × 2 requests = 4
        let registry = spawn_registry(vec![404, 404, 404, 404], 4);

        // When: running preflight with --policy fast (no token)
        let out = shipper_cmd()
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

        // Then: token not detected, exit code 0
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false"),
            "Expected token not detected in output, got:\n{stdout}"
        );

        registry.join();
    }
}

mod preflight_fails_with_dirty_git_tree {
    use super::*;

    // Scenario: Preflight fails with dirty git tree
    //   Given a valid registry token is configured
    //   And the git working tree has uncommitted changes
    //   When I run "shipper preflight" without "--allow-dirty"
    //   Then the exit code is non-zero
    //   And the error message contains "dirty"
    #[test]
    fn given_non_git_directory_when_preflight_without_allow_dirty_then_fails() {
        // Given: a workspace in a non-git temp directory (simulates dirty/missing git)
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // When: running preflight WITHOUT --allow-dirty
        // (no git repo → git cleanliness check fails)
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--skip-ownership-check")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env("CARGO_REGISTRY_TOKEN", "fake-token-for-test")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .failure()
            .stderr(contains("git"));
    }
}

mod strict_ownership_fails_without_token {
    use super::*;

    // Scenario: Strict ownership check fails without token
    //   Given no registry token is configured
    //   And the registry returns "not found" for all crates
    //   When I run "shipper preflight" with "--strict-ownership"
    //   Then the exit code is non-zero
    //   And the error message mentions token or ownership
    #[test]
    fn given_no_token_when_strict_ownership_then_fails_with_error() {
        // Given: no token, --strict-ownership
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // When: running preflight with --strict-ownership but no token
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--allow-dirty")
            .arg("--strict-ownership")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .failure()
            .stderr(contains("strict ownership requested but no token found"));
    }
}

mod balanced_policy_ignores_strict_ownership {
    use super::*;

    // Scenario: Balanced policy ignores strict ownership requirement
    //   Given no registry token is configured
    //   And the registry returns "not found" for all crates
    //   When I run "shipper preflight" with "--policy balanced --strict-ownership --no-verify"
    //   Then the exit code is 0
    //   And the preflight report shows token not detected
    #[test]
    fn given_no_token_when_balanced_with_strict_ownership_then_succeeds() {
        // Given: no token, registry 404 for all
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // balanced + --no-verify: no dry-run, no ownership check (overrides --strict-ownership)
        // 2 crates × 2 requests = 4
        let registry = spawn_registry(vec![404, 404, 404, 404], 4);

        // When: running preflight with --policy balanced --strict-ownership --no-verify
        let out = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("balanced")
            .arg("--strict-ownership")
            .arg("--no-verify")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then: exit 0, token not detected
        // (balanced policy overrides strict_ownership to false, so no token error)
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false"),
            "Expected token not detected in output, got:\n{stdout}"
        );

        registry.join();
    }
}
