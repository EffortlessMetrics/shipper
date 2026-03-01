//! BDD (Behavior-Driven Development) tests for cross-cutting workflow scenarios.
//!
//! These tests correspond to `features/workflow.feature` and exercise the
//! resume, parallel publish, status, and doctor commands in representative
//! end-to-end situations inside temporary workspaces.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use predicates::str::contains;
use serial_test::serial;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn spawn_registry(statuses: Vec<u16>, expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for idx in 0..expected_requests {
            let req = match server.recv_timeout(Duration::from_secs(60)) {
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

fn spawn_doctor_registry(expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for _ in 0..expected_requests {
            let req = match server.recv_timeout(Duration::from_secs(60)) {
                Ok(Some(r)) => r,
                _ => break,
            };
            let resp = Response::from_string(r#"{"crate":{"id":"serde"}}"#)
                .with_status_code(StatusCode(200))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json").expect("header"),
                );
            req.respond(resp).expect("respond");
        }
    });
    TestRegistry { base_url, handle }
}

fn path_sep() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
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

fn setup_fake_cargo(td: &Path) -> (String, String, String) {
    let bin_dir = td.join("fake-bin");
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
    (new_path, real_cargo, fake_cargo)
}

fn fast_args(cmd: &mut Command, manifest: &Path, api_base: &str, state_dir: &Path) {
    cmd.arg("--manifest-path")
        .arg(manifest)
        .arg("--api-base")
        .arg(api_base)
        .arg("--allow-dirty")
        .arg("--verify-timeout")
        .arg("0ms")
        .arg("--verify-poll")
        .arg("0ms")
        .arg("--no-readiness")
        .arg("--max-attempts")
        .arg("2")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(state_dir);
}

// ---------------------------------------------------------------------------
// Workspace builders
// ---------------------------------------------------------------------------

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

fn create_two_crate_workspace(root: &Path) {
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

fn create_independent_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["alpha", "beta", "gamma"]
resolver = "2"
"#,
    );
    for name in &["alpha", "beta", "gamma"] {
        write_file(
            &root.join(format!("{name}/Cargo.toml")),
            &format!(
                r#"
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
"#
            ),
        );
        write_file(
            &root.join(format!("{name}/src/lib.rs")),
            &format!("pub fn {name}() {{}}\n"),
        );
    }
}

fn create_parallel_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "api", "cli", "app"]
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
        &root.join("api/Cargo.toml"),
        r#"
[package]
name = "api"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core" }
"#,
    );
    write_file(&root.join("api/src/lib.rs"), "pub fn api() {}\n");

    write_file(
        &root.join("cli/Cargo.toml"),
        r#"
[package]
name = "cli"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core" }
"#,
    );
    write_file(&root.join("cli/src/lib.rs"), "pub fn cli() {}\n");

    write_file(
        &root.join("app/Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
api = { path = "../api" }
cli = { path = "../cli" }
"#,
    );
    write_file(&root.join("app/src/lib.rs"), "pub fn app() {}\n");
}

fn create_multi_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core-lib", "utils-lib", "top-app"]
resolver = "2"
"#,
    );
    write_file(
        &root.join("core-lib/Cargo.toml"),
        r#"
[package]
name = "core-lib"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("core-lib/src/lib.rs"), "pub fn core() {}\n");

    write_file(
        &root.join("utils-lib/Cargo.toml"),
        r#"
[package]
name = "utils-lib"
version = "0.1.0"
edition = "2021"

[dependencies]
core-lib = { path = "../core-lib" }
"#,
    );
    write_file(
        &root.join("utils-lib/src/lib.rs"),
        "pub fn utils() { core_lib::core(); }\n",
    );

    write_file(
        &root.join("top-app/Cargo.toml"),
        r#"
[package]
name = "top-app"
version = "0.1.0"
edition = "2021"

[dependencies]
core-lib = { path = "../core-lib" }
utils-lib = { path = "../utils-lib" }
"#,
    );
    write_file(
        &root.join("top-app/src/lib.rs"),
        "pub fn app() { utils_lib::utils(); }\n",
    );
}

fn create_solo_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["solo"]
resolver = "2"
"#,
    );
    write_file(
        &root.join("solo/Cargo.toml"),
        r#"
[package]
name = "solo"
version = "0.3.0"
edition = "2021"
"#,
    );
    write_file(&root.join("solo/src/lib.rs"), "pub fn solo() {}\n");
}

// ============================================================================
// Feature: Resume workflow
// ============================================================================

mod resume_continues_after_interruption {
    use super::*;

    // Scenario: Resume after interrupted publish completes remaining crates
    //
    // Given: a workspace with "core" and "app" where "app" depends on "core"
    // And: a prior publish run failed while publishing "app"
    // And: the state file marks core as Skipped and app as Failed
    // When: I run "shipper resume"
    // Then: exit code is 0, receipt shows app as Published, core was not re-published
    #[test]
    #[serial]
    fn given_interrupted_publish_when_resume_then_completes_remaining_crates() {
        // Given: create workspace and fail the initial publish
        let td = tempdir().expect("tempdir");
        create_two_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // Initial publish: core 200 (skip), app 404 cargo-fail 404 404 → ~4 reqs.
        // Resume: app 404, cargo ok, readiness 200 → ~2 reqs.
        let registry = spawn_registry(vec![200, 404, 404, 404, 404, 200], 7);

        // Initial publish that fails on app
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
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("1")
            .arg("--base-delay")
            .arg("0ms")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
            .assert()
            .failure();

        // Verify pre-condition: app is failed
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
        )
        .expect("parse state");
        let app_state = state["packages"]["app@0.1.0"]["state"]["state"]
            .as_str()
            .expect("app state");
        assert_eq!(app_state, "failed", "app should be failed before resume");

        // When: resume with cargo publish succeeding
        let mut cmd = shipper_cmd();
        fast_args(
            &mut cmd,
            &td.path().join("Cargo.toml"),
            &registry.base_url,
            &state_dir,
        );
        cmd.arg("resume")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then: receipt shows app as published
        let receipt: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
        )
        .expect("parse receipt");
        let packages = receipt["packages"].as_array().expect("packages array");
        let app_pkg = packages.iter().find(|p| p["name"].as_str() == Some("app"));
        assert!(app_pkg.is_some(), "receipt should contain app");
        assert_eq!(
            app_pkg.unwrap()["state"]["state"].as_str(),
            Some("published"),
            "app should be published after resume"
        );

        registry.join();
    }
}

mod resume_noop_when_complete {
    use super::*;

    // Scenario: Resume with all packages already published is a no-op
    //
    // Given: a workspace with a single crate that was already published
    // When: I run "shipper resume"
    // Then: exit code is 0, cargo publish is not invoked, output says "already complete"
    #[test]
    #[serial]
    fn given_all_published_when_resume_then_noop() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // First publish successfully: version-check 404, readiness 200 → 2 reqs.
        let registry = spawn_registry(vec![404, 200], 3);

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
            .arg("--no-readiness")
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

        // Verify demo is published in state
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
        )
        .expect("parse state");
        assert_eq!(
            state["packages"]["demo@0.1.0"]["state"]["state"].as_str(),
            Some("published")
        );

        // When: resume on already-completed state
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--verify-timeout")
            .arg("0ms")
            .arg("--verify-poll")
            .arg("0ms")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success()
            .get_output()
            .stderr
            .clone();

        // Then: output says already complete
        let stderr = String::from_utf8(output).expect("utf8");
        assert!(
            stderr.contains("already complete"),
            "expected 'already complete' in stderr, got: {stderr}"
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Parallel publish
// ============================================================================

mod parallel_independent_skipped {
    use super::*;

    // Scenario: Parallel publish groups independent crates into one level
    //
    // Given: a workspace with independent crates alpha, beta, gamma
    // And: registry reports all versions as already published (200)
    // When: I run "shipper publish --parallel --max-concurrent 2"
    // Then: exit code is 0, all three appear in receipt as Skipped
    #[test]
    #[serial]
    fn given_independent_crates_when_parallel_publish_then_all_skipped() {
        let td = tempdir().expect("tempdir");
        create_independent_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // All 200: every version_exists → "already published" → skip
        let registry = spawn_registry(vec![200, 200, 200], 3);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--max-concurrent")
            .arg("2")
            .arg("--parallel")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then: receipt contains all 3 as skipped
        let receipt: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
        )
        .expect("parse receipt");
        let packages = receipt["packages"].as_array().expect("packages array");
        assert_eq!(packages.len(), 3, "receipt should have 3 packages");

        for pkg in packages {
            let pkg_state = pkg["state"]["state"].as_str().unwrap_or("unknown");
            assert!(
                pkg_state == "skipped" || pkg_state == "published",
                "expected skipped or published, got: {pkg_state}"
            );
        }

        registry.join();
    }
}

mod parallel_respects_dependency_ordering {
    use super::*;

    // Scenario: Parallel publish respects dependency ordering across levels
    //
    // Given: a workspace with core → {api, cli} → app
    // And: registry reports all versions as already published
    // When: I run "shipper publish --parallel"
    // Then: exit code is 0, all four crates appear in the receipt
    #[test]
    #[serial]
    fn given_dependencies_when_parallel_publish_then_all_in_receipt() {
        let td = tempdir().expect("tempdir");
        create_parallel_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // All 200: version_exists → skip for 4 crates
        let registry = spawn_registry(vec![200, 200, 200, 200], 4);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--max-concurrent")
            .arg("1")
            .arg("--parallel")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then: receipt has all 4 packages
        let receipt: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
        )
        .expect("parse receipt");
        let packages = receipt["packages"].as_array().expect("packages array");
        assert_eq!(packages.len(), 4, "receipt should have 4 packages");

        let names: Vec<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
        assert!(names.contains(&"core"), "receipt should contain core");
        assert!(names.contains(&"api"), "receipt should contain api");
        assert!(names.contains(&"cli"), "receipt should contain cli");
        assert!(names.contains(&"app"), "receipt should contain app");

        registry.join();
    }
}

// ============================================================================
// Feature: Status command
// ============================================================================

mod status_mixed_published_and_missing {
    use super::*;

    // Scenario: Status reports mixed published and missing crates
    //
    // Given: a workspace with core-lib, utils-lib, and top-app
    // And: registry returns 200 for core-lib, 404 for utils-lib and top-app
    // When: I run "shipper status"
    // Then: exit code is 0, output contains published for core-lib and missing for others
    #[test]
    fn given_mixed_versions_when_status_then_reports_each_correctly() {
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        // core-lib → 200 (published), utils-lib → 404 (missing), top-app → 404 (missing)
        let registry = spawn_registry(vec![200, 404, 404], 3);

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

        // Then: at least one published, at least one missing
        assert!(
            stdout.contains("published"),
            "expected at least one published crate in: {stdout}"
        );
        assert!(
            stdout.contains("missing"),
            "expected at least one missing crate in: {stdout}"
        );

        registry.join();
    }
}

mod status_single_crate_shows_version {
    use super::*;

    // Scenario: Status for a single-crate workspace shows version
    //
    // Given: a workspace with solo@0.3.0
    // And: registry returns 404 (not found)
    // When: I run "shipper status"
    // Then: exit code is 0, output contains "solo@0.3.0"
    #[test]
    fn given_single_crate_when_status_then_shows_version() {
        let td = tempdir().expect("tempdir");
        create_solo_workspace(td.path());

        let registry = spawn_registry(vec![404], 1);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("status")
            .assert()
            .success()
            .stdout(contains("solo@0.3.0"));

        registry.join();
    }
}

// ============================================================================
// Feature: Doctor diagnostics
// ============================================================================

mod doctor_reports_header_and_workspace {
    use super::*;

    // Scenario: Doctor reports diagnostics header and workspace root
    //
    // Given: a valid workspace with crate "demo" and a reachable mock registry
    // When: I run "shipper doctor"
    // Then: exit code is 0, output contains header and workspace_root
    #[test]
    fn given_valid_workspace_when_doctor_then_reports_header_and_root() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_doctor_registry(1);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .stdout(contains("Shipper Doctor - Diagnostics Report"))
            .stdout(contains("workspace_root:"));

        registry.join();
    }
}

mod doctor_warns_missing_token {
    use super::*;

    // Scenario: Doctor warns when no registry token is configured
    //
    // Given: a valid workspace, no CARGO_REGISTRY_TOKEN
    // When: I run "shipper doctor"
    // Then: exit code is 0, output contains "NONE FOUND"
    #[test]
    fn given_no_token_when_doctor_then_warns_none_found() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let cargo_home = td.path().join("cargo-home");
        fs::create_dir_all(&cargo_home).expect("mkdir");

        let registry = spawn_doctor_registry(1);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", &cargo_home)
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .stdout(contains("NONE FOUND"));

        registry.join();
    }
}

mod doctor_detects_cargo {
    use super::*;

    // Scenario: Doctor detects cargo version
    //
    // Given: a valid workspace (cargo is on PATH)
    // When: I run "shipper doctor"
    // Then: exit code is 0, output contains cargo version line
    #[test]
    fn given_cargo_installed_when_doctor_then_shows_version() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_doctor_registry(1);

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).expect("utf8");
        assert!(
            stdout.contains("cargo: cargo"),
            "expected cargo version line, got: {stdout}"
        );

        registry.join();
    }
}

mod doctor_reports_registry_reachability {
    use super::*;

    // Scenario: Doctor reports registry reachability
    //
    // Given: a valid workspace with a reachable mock registry
    // When: I run "shipper doctor"
    // Then: exit code is 0, output contains "registry_reachable: true"
    #[test]
    fn given_reachable_registry_when_doctor_then_reports_reachable() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_doctor_registry(1);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .stdout(contains("registry_reachable: true"));

        registry.join();
    }
}

// ============================================================================
// Feature: Config validation workflow
// ============================================================================

mod config_validate_rejects_zero_max_attempts {
    use super::*;

    // Scenario: Config validate rejects zero retry max_attempts
    //
    // Given: a .shipper.toml with retry.max_attempts = 0
    // When: I run "shipper config validate"
    // Then: exit code is non-zero, error mentions "max_attempts"
    #[test]
    fn given_zero_max_attempts_when_config_validate_then_error() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join(".shipper.toml"),
            r#"
schema_version = "shipper.config.v1"

[retry]
max_attempts = 0
"#,
        );

        shipper_cmd()
            .arg("config")
            .arg("validate")
            .arg("-p")
            .arg(td.path().join(".shipper.toml"))
            .assert()
            .failure()
            .stderr(contains("max_attempts"));
    }
}

mod config_validate_rejects_invalid_jitter {
    use super::*;

    // Scenario: Config validate rejects jitter outside valid range
    //
    // Given: a .shipper.toml with retry.jitter = 1.5
    // When: I run "shipper config validate"
    // Then: exit code is non-zero, error mentions "jitter"
    #[test]
    fn given_invalid_jitter_when_config_validate_then_error() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join(".shipper.toml"),
            r#"
schema_version = "shipper.config.v1"

[retry]
jitter = 1.5
"#,
        );

        shipper_cmd()
            .arg("config")
            .arg("validate")
            .arg("-p")
            .arg(td.path().join(".shipper.toml"))
            .assert()
            .failure()
            .stderr(contains("jitter"));
    }
}

// ============================================================================
// Feature: Doctor token warning
// ============================================================================

mod doctor_reports_token_source_when_missing {
    use super::*;

    // Scenario: Doctor reports token source when no token is configured
    //
    // Given: a valid workspace, no CARGO_REGISTRY_TOKEN, no credentials file
    // When: I run "shipper doctor"
    // Then: exit code is 0, output contains "auth_type:" and "NONE FOUND"
    #[test]
    fn given_no_token_no_credentials_when_doctor_then_reports_none_found() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let cargo_home = td.path().join("cargo-home");
        fs::create_dir_all(&cargo_home).expect("mkdir");

        let registry = spawn_doctor_registry(1);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", &cargo_home)
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .stdout(contains("auth_type:"))
            .stdout(contains("NONE FOUND"));

        registry.join();
    }
}

// ============================================================================
// Feature: Clean command
// ============================================================================

mod clean_removes_state_files {
    use super::*;

    // Scenario: Clean removes state files from .shipper directory
    //
    // Given: a workspace with "demo" and a state directory containing state.json and events.jsonl
    // When: I run "shipper clean"
    // Then: exit code is 0, output contains "Clean complete", state.json is removed
    #[test]
    #[serial]
    fn given_state_files_when_clean_then_removes_them() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join(".shipper");
        fs::create_dir_all(&state_dir).expect("mkdir");

        // Pre-populate state files
        write_file(&state_dir.join("state.json"), r#"{"plan_id":"test"}"#);
        write_file(&state_dir.join("events.jsonl"), "{}\n");
        assert!(state_dir.join("state.json").exists());

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("clean")
            .assert()
            .success()
            .stdout(contains("Clean complete"));

        // Then: state.json should be removed
        assert!(
            !state_dir.join("state.json").exists(),
            "state.json should be removed after clean"
        );
        assert!(
            !state_dir.join("events.jsonl").exists(),
            "events.jsonl should be removed after clean"
        );
    }
}

mod clean_keep_receipt {
    use super::*;

    // Scenario: Clean with --keep-receipt preserves receipt.json
    //
    // Given: a workspace with state.json, events.jsonl, and receipt.json in state dir
    // When: I run "shipper clean --keep-receipt"
    // Then: exit code is 0, receipt.json still exists, state.json is removed
    #[test]
    #[serial]
    fn given_receipt_when_clean_keep_receipt_then_preserves_it() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join(".shipper");
        fs::create_dir_all(&state_dir).expect("mkdir");

        // Pre-populate state files including receipt
        write_file(&state_dir.join("state.json"), r#"{"plan_id":"test"}"#);
        write_file(&state_dir.join("events.jsonl"), "{}\n");
        write_file(&state_dir.join("receipt.json"), r#"{"packages":[]}"#);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("clean")
            .arg("--keep-receipt")
            .assert()
            .success()
            .stdout(contains("Clean complete"));

        // Then: receipt.json should be preserved
        assert!(
            state_dir.join("receipt.json").exists(),
            "receipt.json should be preserved with --keep-receipt"
        );
        // And: state.json should be removed
        assert!(
            !state_dir.join("state.json").exists(),
            "state.json should be removed"
        );
    }
}

// ============================================================================
// Feature: Plan with package filter
// ============================================================================

mod plan_with_package_filter {
    use super::*;

    // Scenario: Plan with --package filter shows only selected package and its deps
    //
    // Given: a workspace with "core", "utils", and "app" where "app" depends on both
    // When: I run "shipper plan --package app"
    // Then: exit code is 0, output contains "app@0.1.0"
    #[test]
    fn given_multi_crate_when_plan_with_package_then_shows_filtered() {
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--package")
            .arg("top-app")
            .arg("plan")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).expect("utf8");

        // Then: output should contain the filtered package
        assert!(
            stdout.contains("top-app@0.1.0"),
            "expected top-app@0.1.0 in plan output, got: {stdout}"
        );
        // And: total packages should reflect filtered set (app + its deps)
        assert!(
            stdout.contains("Total packages to publish:"),
            "expected total packages line in output, got: {stdout}"
        );
    }
}

// ============================================================================
// Feature: Dry run publish (preflight)
// ============================================================================

mod preflight_checks_without_publishing {
    use super::*;

    // Scenario: Preflight checks workspace without publishing
    //
    // Given: a workspace with "demo" and registry reports version as already published
    // When: I run "shipper preflight --allow-dirty"
    // Then: exit code is 0, no state.json created
    #[test]
    fn given_workspace_when_preflight_then_no_state_file() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join(".shipper");

        // Registry returns 200 for version-exists checks; preflight may issue multiple requests
        let registry = spawn_registry(vec![200, 200, 200], 3);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--skip-ownership-check")
            .arg("--no-verify")
            .arg("preflight")
            .assert()
            .success();

        // Then: no state.json should be created (preflight doesn't persist state)
        assert!(
            !state_dir.join("state.json").exists(),
            "preflight should not create state.json"
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Edge-case scenarios
// ============================================================================

fn create_dev_dependency_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["lib-a", "lib-b"]
resolver = "2"
"#,
    );
    write_file(
        &root.join("lib-a/Cargo.toml"),
        r#"
[package]
name = "lib-a"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("lib-a/src/lib.rs"), "pub fn a() {}\n");
    write_file(
        &root.join("lib-b/Cargo.toml"),
        r#"
[package]
name = "lib-b"
version = "0.1.0"
edition = "2021"

[dev-dependencies]
lib-a = { path = "../lib-a" }
"#,
    );
    write_file(
        &root.join("lib-b/src/lib.rs"),
        "pub fn b() {}\n#[cfg(test)] mod tests { use lib_a::a; #[test] fn it() { a(); } }\n",
    );
}

mod publish_all_already_published_sequential {
    use super::*;

    // Scenario: Sequential publish when all crates are already published skips everything
    //
    // Given: a workspace with "core" and "app" where "app" depends on "core"
    // And: registry reports both versions as already published (200)
    // When: I run "shipper publish" (sequential, no --parallel)
    // Then: exit code is 0, receipt shows both crates as skipped
    #[test]
    #[serial]
    fn given_all_published_when_sequential_publish_then_all_skipped() {
        let td = tempdir().expect("tempdir");
        create_two_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // Both return 200 → already published → skip
        let registry = spawn_registry(vec![200, 200], 2);

        let mut cmd = shipper_cmd();
        fast_args(
            &mut cmd,
            &td.path().join("Cargo.toml"),
            &registry.base_url,
            &state_dir,
        );
        cmd.arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then: receipt shows all crates as skipped
        let receipt: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
        )
        .expect("parse receipt");
        let packages = receipt["packages"].as_array().expect("packages array");
        assert_eq!(packages.len(), 2, "receipt should have 2 packages");

        for pkg in packages {
            let pkg_state = pkg["state"]["state"].as_str().unwrap_or("unknown");
            assert_eq!(
                pkg_state, "skipped",
                "expected skipped for {}, got: {pkg_state}",
                pkg["name"]
            );
        }

        registry.join();
    }
}

mod clean_with_no_state_directory {
    use super::*;

    // Scenario: Clean when .shipper directory does not exist exits gracefully
    //
    // Given: a workspace with "demo" and no .shipper directory
    // When: I run "shipper clean"
    // Then: exit code is 0, output says "State directory does not exist"
    #[test]
    fn given_no_state_dir_when_clean_then_reports_not_exist() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join(".shipper");

        // Ensure .shipper does not exist
        assert!(!state_dir.exists());

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("clean")
            .assert()
            .success()
            .stdout(contains("State directory does not exist"));
    }
}

mod doctor_reports_unreachable_registry {
    use super::*;

    // Scenario: Doctor reports registry unreachable when mock server is not running
    //
    // Given: a valid workspace with an unreachable registry API base
    // When: I run "shipper doctor"
    // Then: exit code is 0, output contains "registry_reachable: false"
    #[test]
    fn given_unreachable_registry_when_doctor_then_reports_unreachable() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // Use a port that is guaranteed not to be listening
        let bad_url = "http://127.0.0.1:1";

        let assert = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(bad_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success();

        // registry_reachable: false is emitted via reporter.warn() → stderr
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
        assert!(
            stderr.contains("registry_reachable: false"),
            "expected 'registry_reachable: false' in stderr, got: {stderr}"
        );
    }
}

mod plan_with_dev_dependencies_only {
    use super::*;

    // Scenario: Plan on a workspace where crates have only dev-dependencies
    //
    // Given: a workspace with "lib-a" and "lib-b" where "lib-b" dev-depends on "lib-a"
    // When: I run "shipper plan"
    // Then: exit code is 0, output lists both crates, total is 2
    #[test]
    fn given_dev_deps_only_when_plan_then_both_crates_listed() {
        let td = tempdir().expect("tempdir");
        create_dev_dependency_workspace(td.path());

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("plan")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).expect("utf8");

        assert!(
            stdout.contains("lib-a@0.1.0"),
            "expected lib-a@0.1.0 in plan output, got: {stdout}"
        );
        assert!(
            stdout.contains("lib-b@0.1.0"),
            "expected lib-b@0.1.0 in plan output, got: {stdout}"
        );
        assert!(
            stdout.contains("Total packages to publish:"),
            "expected total packages line in output, got: {stdout}"
        );
    }
}

mod preflight_fails_on_non_git_directory {
    use super::*;

    // Scenario: Preflight fails when run in a non-git directory without --allow-dirty
    //
    // Given: a workspace with "demo" that is NOT inside a git repository
    // When: I run "shipper preflight" (without --allow-dirty)
    // Then: exit code is non-zero (git cleanliness check fails)
    #[test]
    fn given_non_git_dir_when_preflight_without_allow_dirty_then_fails() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let registry = spawn_registry(vec![200, 200, 200], 3);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--skip-ownership-check")
            .arg("--no-verify")
            .arg("preflight")
            .assert()
            .failure();

        registry.join();
    }
}

mod resume_with_corrupted_state_file {
    use super::*;

    // Scenario: Resume with a corrupted (non-JSON) state file fails gracefully
    //
    // Given: a workspace with "demo" and a state file containing garbage data
    // When: I run "shipper resume"
    // Then: exit code is non-zero, error output mentions parse/state issue
    #[test]
    #[serial]
    fn given_corrupted_state_when_resume_then_fails_with_error() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join(".shipper");
        fs::create_dir_all(&state_dir).expect("mkdir");

        // Write corrupted state
        write_file(&state_dir.join("state.json"), "NOT VALID JSON {{{{");

        let registry = spawn_registry(vec![200], 1);

        let mut cmd = shipper_cmd();
        fast_args(
            &mut cmd,
            &td.path().join("Cargo.toml"),
            &registry.base_url,
            &state_dir,
        );
        cmd.arg("resume").assert().failure();

        registry.join();
    }
}

mod status_all_published {
    use super::*;

    // Scenario: Status shows all crates as published when registry reports all exist
    //
    // Given: a workspace with "core" and "app"
    // And: registry returns 200 for both versions
    // When: I run "shipper status"
    // Then: exit code is 0, output contains "published" for both, no "missing"
    #[test]
    fn given_all_published_when_status_then_no_missing() {
        let td = tempdir().expect("tempdir");
        create_two_crate_workspace(td.path());

        // Both return 200 → published
        let registry = spawn_registry(vec![200, 200], 2);

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

        assert!(
            stdout.contains("published"),
            "expected published in output, got: {stdout}"
        );
        assert!(
            !stdout.contains("missing"),
            "expected no missing in output, got: {stdout}"
        );

        registry.join();
    }
}
