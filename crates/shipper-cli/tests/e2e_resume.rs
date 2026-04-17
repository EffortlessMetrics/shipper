//! End-to-end tests for the `shipper resume` command.
//!
//! Tests cover: missing state, valid resume, completed state, plan mismatch,
//! custom --state-dir, and partial publish resume.

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

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
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

/// Write a mock state.json into the given directory.
fn write_state_json(state_dir: &Path, json: &str) {
    fs::create_dir_all(state_dir).expect("mkdir state dir");
    fs::write(state_dir.join("state.json"), json).expect("write state.json");
}

// ============================================================================
// Test 1: Resume with no existing state file shows appropriate error
// ============================================================================

#[test]
fn resume_no_state_file_shows_error() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let state_dir = td.path().join("empty-state");
    fs::create_dir_all(&state_dir).expect("mkdir");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .assert()
        .failure()
        .stderr(contains("no existing state found"));
}

// ============================================================================
// Test 2: Resume with valid state file continues from where it left off
// ============================================================================

#[test]
#[serial]
fn resume_continues_from_failed_state() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let state_dir = td.path().join(".shipper");

    // Single registry for both publish and resume so plan_id stays consistent.
    // Publish (fail): version-check 404, post-failure check 404, final-chance 404 → 3 requests
    // Resume (success): version-check 404, readiness 200 → 2 requests
    // Total: 5 requests
    let registry = spawn_registry(vec![404, 404, 404, 404, 200], 5);

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

    // Verify state.json exists with non-published state
    let state_path = state_dir.join("state.json");
    assert!(state_path.exists(), "state.json should exist after failure");

    // Resume: cargo publish now succeeds (exit 0).
    // Use --max-attempts 2 because the state already has attempts=1;
    // the retry loop needs max_attempts > saved attempts to enter.
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
        .arg("2")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success();

    // Receipt should show published
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse");
    let packages = receipt["packages"].as_array().expect("packages array");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"].as_str(), Some("demo"));
    assert_eq!(
        packages[0]["state"]["state"].as_str(),
        Some("published"),
        "resumed package should be published"
    );

    registry.join();
}

// ============================================================================
// Test 3: Resume with completed state file reports already done
// ============================================================================

#[test]
#[serial]
fn resume_with_all_published_state_succeeds() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let state_dir = td.path().join(".shipper");

    // Single registry for both publish and resume.
    // Publish: version-check 404, readiness 200 → 2 requests
    // Resume: all packages already published in state → 0 requests
    // Use expected_requests=3 so the server stays alive through the resume command;
    // the extra slot times out harmlessly via recv_timeout.
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

    // State should show everything published
    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
    )
    .expect("parse");
    let pkgs = state["packages"].as_object().expect("packages");
    let demo = &pkgs["demo@0.1.0"];
    assert_eq!(demo["state"]["state"].as_str(), Some("published"));

    // Resume: everything is already complete, so it should succeed and skip.
    // No registry requests needed since all packages are Published in state.
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

    let stderr = String::from_utf8(output).expect("utf8");
    // Engine logs "already complete" for published packages
    assert!(
        stderr.contains("already complete"),
        "should report already complete, got stderr: {stderr}"
    );

    registry.join();
}

// ============================================================================
// Test 4: Resume detects plan mismatch (different plan_id)
// ============================================================================

#[test]
fn resume_plan_id_mismatch_fails() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let state_dir = td.path().join(".shipper");

    // Write a state.json with a plan_id that won't match the computed plan
    let mock_state = r#"{
        "state_version": "shipper.state.v1",
        "plan_id": "intentionally-wrong-plan-id-12345",
        "registry": {
            "name": "crates-io",
            "api_base": "https://crates.io",
            "index_base": "https://index.crates.io"
        },
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z",
        "packages": {
            "demo@0.1.0": {
                "name": "demo",
                "version": "0.1.0",
                "attempts": 1,
                "state": { "state": "pending" },
                "last_updated_at": "2024-01-01T00:00:00Z"
            }
        }
    }"#;
    write_state_json(&state_dir, mock_state);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--allow-dirty")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .assert()
        .failure()
        .stderr(contains("does not match current plan_id"))
        .stderr(contains("--force-resume"));
}

// ============================================================================
// Test 5: Resume with --state-dir reads from correct location
// ============================================================================

#[test]
fn resume_with_custom_state_dir() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let custom_dir = td.path().join("my-custom-state");

    // No state.json in custom dir → resume should fail citing that directory
    fs::create_dir_all(&custom_dir).expect("mkdir");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&custom_dir)
        .arg("resume")
        .assert()
        .failure()
        .stderr(contains("no existing state found"));

    // Default .shipper dir should not have been created
    assert!(
        !td.path().join(".shipper").exists(),
        "default .shipper should not be created when --state-dir is used"
    );
}

// ============================================================================
// Test 6: Resume after partial publish (one crate published, one pending)
// ============================================================================

#[test]
#[serial]
fn resume_after_partial_publish() {
    let td = tempdir().expect("tempdir");
    create_two_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let state_dir = td.path().join(".shipper");

    // Single registry for both publish and resume.
    // Publish (cargo exit=1 so all cargo publish calls fail):
    //   core version-check 200 (already on registry → skip): 1 request
    //   app version-check 404 (not published), cargo fails: 1 request
    //   app post-failure check 404: 1 request
    //   app final-chance check 404: 1 request
    // Resume (cargo exit=0):
    //   core already skipped in state → 0 requests
    //   app version-check 404, cargo succeeds, readiness 200: 2 requests
    // Total: 6 requests
    // Add an extra buffer slot so the server stays alive between publish and resume.
    let registry = spawn_registry(vec![200, 404, 404, 404, 404, 200], 7);

    // First: publish all crates with cargo failing.
    // core gets skipped (registry says 200 → already published), app fails.
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

    // Verify state: core should be skipped, app should be failed
    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
    )
    .expect("parse state");

    let pkgs = state["packages"].as_object().expect("packages");
    let core_state = pkgs["core@0.1.0"]["state"]["state"]
        .as_str()
        .expect("core state");
    assert!(
        core_state == "skipped" || core_state == "published",
        "core should be skipped/published, got: {core_state}"
    );
    let app_state = pkgs["app@0.1.0"]["state"]["state"]
        .as_str()
        .expect("app state");
    assert_eq!(app_state, "failed", "app should be failed");

    // Resume: core already complete → skip, app retried and succeeds.
    // Use --max-attempts 2 because state already has attempts=1 for app.
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
        .arg("2")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success();

    // Verify final state: both packages should be complete
    let final_state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
    )
    .expect("parse state");
    let final_pkgs = final_state["packages"].as_object().expect("packages");
    let final_core = final_pkgs["core@0.1.0"]["state"]["state"]
        .as_str()
        .expect("core state");
    assert!(
        final_core == "skipped" || final_core == "published",
        "core should still be skipped/published, got: {final_core}"
    );
    let final_app = final_pkgs["app@0.1.0"]["state"]["state"]
        .as_str()
        .expect("app state");
    assert!(
        final_app == "published",
        "app should now be published, got: {final_app}"
    );

    registry.join();
}
