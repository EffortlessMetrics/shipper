//! BDD (Behavior-Driven Development) tests for the shipper publish flow.
//!
//! These tests verify CLI behavior around the `publish` subcommand using
//! Given-When-Then style documentation. Where possible, tests avoid real
//! publishing by using the `plan`/`preflight` commands, fake cargo proxies,
//! and mock registries.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

// ---------------------------------------------------------------------------
// Helpers (mirrors conventions from sibling test files)
// ---------------------------------------------------------------------------

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

fn create_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "utils", "app"]
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

fn create_single_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["demo"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("demo/Cargo.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("demo/src/lib.rs"), "pub fn demo() {}\n");
}

fn create_fake_cargo_proxy(bin_dir: &Path) {
    #[cfg(windows)]
    {
        fs::write(
            bin_dir.join("cargo.cmd"),
            "@echo off\r\nif \"%1\"==\"publish\" (\r\n  if \"%SHIPPER_FAKE_PUBLISH_EXIT%\"==\"\" (exit /b 0) else (exit /b %SHIPPER_FAKE_PUBLISH_EXIT%)\r\n)\r\n\"%REAL_CARGO%\" %*\r\nexit /b %ERRORLEVEL%\r\n",
        )
        .expect("write fake cargo");
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cargo");
        fs::write(
            &path,
            "#!/usr/bin/env sh\nif [ \"$1\" = \"publish\" ]; then\n  exit \"${SHIPPER_FAKE_PUBLISH_EXIT:-0}\"\nfi\n\"$REAL_CARGO\" \"$@\"\n",
        )
        .expect("write fake cargo");
        let mut perms = fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
    }
}

fn path_sep() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

fn fake_cargo_bin_path(bin_dir: &Path) -> String {
    #[cfg(windows)]
    {
        bin_dir.join("cargo.cmd").display().to_string()
    }
    #[cfg(not(windows))]
    {
        bin_dir.join("cargo").display().to_string()
    }
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
            let req = match server.recv_timeout(Duration::from_secs(30)) {
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

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

// ============================================================================
// Feature: Publish Flow – Pre-publish Preview
// ============================================================================

mod publish_preview {
    use super::*;

    // Scenario: workspace with unpublished crates shows plan without publishing
    //
    // Given: A workspace with unpublished crates
    // When:  Running the `plan` command (non-destructive preview of publish)
    // Then:  All crates are listed in dependency order without side-effects
    #[test]
    fn given_unpublished_workspace_when_plan_then_shows_publish_order_without_side_effects() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // When
        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("plan")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then – plan lists all packages in dependency order
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(stdout.contains("core@0.1.0"), "core should appear in plan");
        assert!(
            stdout.contains("utils@0.1.0"),
            "utils should appear in plan"
        );
        assert!(stdout.contains("app@0.1.0"), "app should appear in plan");

        // No state directory should be created (plan is non-destructive)
        assert!(
            !td.path().join(".shipper").exists(),
            "plan should not create .shipper state directory"
        );
    }

    // Scenario: preflight reports publish-readiness without actual publishing
    //
    // Given: A workspace with unpublished crates
    // When:  Running `preflight` (the non-destructive publish check)
    // Then:  Reports readiness checks and succeeds without side-effects
    #[test]
    fn given_unpublished_workspace_when_preflight_then_reports_readiness_without_publishing() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // 3 crates × (version check + new-crate check) = 6 requests
        let registry = spawn_registry(vec![404], 6);

        // When
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

        // Then
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("core") && stdout.contains("utils") && stdout.contains("app"),
            "preflight should mention all packages"
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Publish Flow – Missing / Invalid Manifests
// ============================================================================

mod manifest_errors {
    use super::*;

    // Scenario: no Cargo.toml at all
    //
    // Given: An empty directory without Cargo.toml
    // When:  Running publish
    // Then:  Fails with a clear error message
    #[test]
    fn given_no_cargo_toml_when_publish_then_fails_with_clear_error() {
        // Given
        let td = tempdir().expect("tempdir");

        // When / Then
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("publish")
            .assert()
            .failure()
            .stderr(contains("Cargo.toml").or(contains("manifest")));
    }

    // Scenario: invalid manifest path
    //
    // Given: A path that does not exist
    // When:  Running publish with --manifest-path pointing to it
    // Then:  Fails appropriately
    #[test]
    fn given_invalid_manifest_path_when_publish_then_fails_appropriately() {
        // Given / When / Then
        shipper_cmd()
            .arg("--manifest-path")
            .arg("nonexistent/path/Cargo.toml")
            .arg("publish")
            .assert()
            .failure();
    }

    // Scenario: manifest path to a file that is not valid TOML
    //
    // Given: A directory with an invalid Cargo.toml
    // When:  Running publish with --manifest-path
    // Then:  Fails with an error
    #[test]
    fn given_malformed_manifest_when_publish_then_fails() {
        // Given
        let td = tempdir().expect("tempdir");
        write_file(&td.path().join("Cargo.toml"), "this is not valid TOML {{{{");

        // When / Then
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("publish")
            .assert()
            .failure();
    }
}

// ============================================================================
// Feature: Publish Flow – Package Filtering
// ============================================================================

mod package_filtering {
    use super::*;

    // Scenario: --package restricts plan to a single crate
    //
    // Given: A workspace with multiple crates
    // When:  Running plan with --package <name>
    // Then:  Only that package is listed in the plan
    #[test]
    fn given_workspace_when_plan_with_package_then_only_targets_that_package() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // When
        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--package")
            .arg("core")
            .arg("plan")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(stdout.contains("core@0.1.0"), "core should be in plan");
        assert!(
            !stdout.contains("utils@0.1.0"),
            "utils should NOT be in plan"
        );
        assert!(!stdout.contains("app@0.1.0"), "app should NOT be in plan");
    }

    // Scenario: --package restricts preflight to a single crate
    //
    // Given: A workspace with multiple crates
    // When:  Running preflight with --package <name>
    // Then:  Only that package is checked
    #[test]
    fn given_workspace_when_preflight_with_package_then_only_targets_that_package() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // Only 1 crate × (version check + new-crate check) = 2 requests
        let registry = spawn_registry(vec![404], 2);

        // When
        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
            .arg("--package")
            .arg("core")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(stdout.contains("core"), "core should appear in preflight");
        assert!(
            !stdout.contains("utils"),
            "utils should NOT appear in preflight"
        );

        registry.join();
    }

    // Scenario: --package with a nonexistent crate name fails
    //
    // Given: A workspace
    // When:  Running plan with --package pointing to a nonexistent crate
    // Then:  Fails with an appropriate error
    #[test]
    fn given_workspace_when_package_flag_with_nonexistent_crate_then_fails() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // When / Then
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--package")
            .arg("nonexistent-crate-xyz")
            .arg("plan")
            .assert()
            .failure();
    }
}

// ============================================================================
// Feature: Publish Flow – State Directory
// ============================================================================

mod state_directory {
    use super::*;

    // Scenario: --state-dir writes state to a custom directory
    //
    // Given: A workspace with a single crate
    // When:  Running publish with --state-dir pointing to a custom location
    // Then:  State and receipt files are created in that directory
    #[test]
    fn given_workspace_when_publish_with_state_dir_then_state_written_to_custom_dir() {
        // Given
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        create_fake_cargo_proxy(&bin_dir);

        let old_path = std::env::var("PATH").unwrap_or_default();
        let mut new_path = bin_dir.display().to_string();
        if !old_path.is_empty() {
            new_path.push_str(path_sep());
            new_path.push_str(&old_path);
        }
        let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let fake_cargo = fake_cargo_bin_path(&bin_dir);

        let custom_state_dir = td.path().join("my-custom-state");

        // Mock registry: version check 404 (not published), then readiness 200
        let registry = spawn_registry(vec![404, 200], 2);

        // When
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--verify-timeout")
            .arg("0ms")
            .arg("--verify-poll")
            .arg("0ms")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&custom_state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then: state files should exist in the custom directory
        assert!(
            custom_state_dir.exists(),
            "custom state dir should be created"
        );

        let receipt_path = custom_state_dir.join("receipt.json");
        assert!(
            receipt_path.exists(),
            "receipt.json should be in custom state dir"
        );

        let receipt_json = fs::read_to_string(&receipt_path).expect("read receipt");
        let receipt: serde_json::Value = serde_json::from_str(&receipt_json).expect("parse json");
        assert!(
            receipt.get("plan_id").is_some(),
            "receipt should have plan_id"
        );

        registry.join();
    }

    // Scenario: publish creates state.json alongside receipt.json
    //
    // Given: A workspace
    // When:  Running publish with --state-dir
    // Then:  Both state.json and receipt.json exist in the state directory
    #[test]
    fn given_workspace_when_publish_then_state_json_and_receipt_json_created() {
        // Given
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        create_fake_cargo_proxy(&bin_dir);

        let old_path = std::env::var("PATH").unwrap_or_default();
        let mut new_path = bin_dir.display().to_string();
        if !old_path.is_empty() {
            new_path.push_str(path_sep());
            new_path.push_str(&old_path);
        }
        let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let fake_cargo = fake_cargo_bin_path(&bin_dir);

        let state_dir = td.path().join("artifacts");
        let registry = spawn_registry(vec![404, 200], 2);

        // When
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--verify-timeout")
            .arg("0ms")
            .arg("--verify-poll")
            .arg("0ms")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then
        assert!(
            state_dir.join("state.json").exists(),
            "state.json should exist"
        );
        assert!(
            state_dir.join("receipt.json").exists(),
            "receipt.json should exist"
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Publish Flow – Full Publish with Fake Cargo
// ============================================================================

mod publish_execution {
    use super::*;

    // Scenario: successful publish of a single-crate workspace
    //
    // Given: A single-crate workspace with a fake cargo proxy
    // When:  Running publish
    // Then:  Succeeds and receipt shows the package as published
    #[test]
    fn given_single_crate_workspace_when_publish_then_receipt_shows_published() {
        // Given
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        create_fake_cargo_proxy(&bin_dir);

        let old_path = std::env::var("PATH").unwrap_or_default();
        let mut new_path = bin_dir.display().to_string();
        if !old_path.is_empty() {
            new_path.push_str(path_sep());
            new_path.push_str(&old_path);
        }
        let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let fake_cargo = fake_cargo_bin_path(&bin_dir);

        // version-check 404 (not yet published), then readiness 200 (visible)
        let registry = spawn_registry(vec![404, 200], 2);

        let state_dir = td.path().join(".shipper");

        // When
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--verify-timeout")
            .arg("0ms")
            .arg("--verify-poll")
            .arg("0ms")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then
        let receipt_path = state_dir.join("receipt.json");
        let receipt_json = fs::read_to_string(&receipt_path).expect("read receipt");
        let receipt: serde_json::Value = serde_json::from_str(&receipt_json).expect("parse");
        let packages = receipt["packages"].as_array().expect("packages array");
        assert_eq!(packages.len(), 1, "should have exactly one package");
        assert_eq!(
            packages[0]["name"].as_str(),
            Some("demo"),
            "package name should be demo"
        );
        assert_eq!(
            packages[0]["state"]["state"].as_str(),
            Some("published"),
            "package should be marked published"
        );

        registry.join();
    }

    // Scenario: publish of multi-crate workspace respects dependency order
    //
    // Given: A workspace with core -> utils -> app
    // When:  Running publish with fake cargo
    // Then:  Receipt lists all packages as published
    #[test]
    fn given_multi_crate_workspace_when_publish_then_all_packages_published() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        create_fake_cargo_proxy(&bin_dir);

        let old_path = std::env::var("PATH").unwrap_or_default();
        let mut new_path = bin_dir.display().to_string();
        if !old_path.is_empty() {
            new_path.push_str(path_sep());
            new_path.push_str(&old_path);
        }
        let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let fake_cargo = fake_cargo_bin_path(&bin_dir);

        // 3 crates × (version-check 404 + readiness 200) = 6 requests
        let registry = spawn_registry(vec![404, 200, 404, 200, 404, 200], 6);

        let state_dir = td.path().join(".shipper");

        // When
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--verify-timeout")
            .arg("0ms")
            .arg("--verify-poll")
            .arg("0ms")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then
        let receipt_json =
            fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt");
        let receipt: serde_json::Value = serde_json::from_str(&receipt_json).expect("parse");
        let packages = receipt["packages"].as_array().expect("packages");
        assert_eq!(packages.len(), 3, "all 3 packages should be in receipt");

        let published_count = packages
            .iter()
            .filter(|p| p["state"]["state"].as_str() == Some("published"))
            .count();
        assert_eq!(published_count, 3, "all 3 packages should be published");

        registry.join();
    }
}
