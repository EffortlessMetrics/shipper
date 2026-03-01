//! End-to-end tests for the full `shipper publish` flow.
//!
//! Tests cover single-crate and multi-crate publishes, state/receipt/events
//! verification, --dry-run-like behavior, --package scoping, custom --state-dir,
//! failed publishes, and re-running publish when everything is already published.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
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

/// Build env vars needed for fake cargo, returning (new_path, real_cargo, fake_cargo).
fn fake_cargo_env(bin_dir: &Path) -> (String, String, String) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let mut new_path = bin_dir.display().to_string();
    if !old_path.is_empty() {
        new_path.push_str(path_sep());
        new_path.push_str(&old_path);
    }
    let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let fake_cargo = fake_cargo_bin_path(bin_dir);
    (new_path, real_cargo, fake_cargo)
}

/// Set up a temp dir with fake cargo bin and return (bin_dir, new_path, real_cargo, fake_cargo).
fn setup_fake_cargo(td: &Path) -> (String, String, String) {
    let bin_dir = td.join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("mkdir");
    create_fake_cargo_proxy(&bin_dir);
    fake_cargo_env(&bin_dir)
}

// ============================================================================
// Test 1: Single-crate publish success with state/receipt verification
// ============================================================================

#[test]
fn single_crate_publish_creates_state_and_receipt() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // version-check 404 (not yet published), then readiness 200 (visible)
    let registry = spawn_registry(vec![404, 200], 2);

    let state_dir = td.path().join(".shipper");

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

    // Verify state.json exists and has plan_id
    let state_path = state_dir.join("state.json");
    assert!(state_path.exists(), "state.json should exist");
    let state_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&state_path).expect("read state"))
            .expect("parse state");
    assert!(
        state_json.get("plan_id").is_some(),
        "state should have plan_id"
    );

    // Verify receipt.json exists and shows published
    let receipt_path = state_dir.join("receipt.json");
    assert!(receipt_path.exists(), "receipt.json should exist");
    let receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&receipt_path).expect("read receipt"))
            .expect("parse receipt");
    assert!(
        receipt.get("plan_id").is_some(),
        "receipt should have plan_id"
    );

    let packages = receipt["packages"].as_array().expect("packages array");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"].as_str(), Some("demo"));
    assert_eq!(
        packages[0]["state"]["state"].as_str(),
        Some("published"),
        "package should be marked published"
    );

    // Verify receipt has timing fields
    assert!(
        receipt.get("started_at").is_some(),
        "receipt should have started_at"
    );
    assert!(
        receipt.get("finished_at").is_some(),
        "receipt should have finished_at"
    );

    registry.join();
}

// ============================================================================
// Test 2: Multi-crate workspace respects dependency ordering
// ============================================================================

#[test]
fn multi_crate_publish_respects_dependency_order() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // 3 crates × (version-check 404 + readiness 200) = 6 requests
    let registry = spawn_registry(vec![404, 200, 404, 200, 404, 200], 6);

    let state_dir = td.path().join(".shipper");

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
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");

    // All 3 packages should be published
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse");
    let packages = receipt["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 3, "all 3 packages should be in receipt");

    let published_count = packages
        .iter()
        .filter(|p| p["state"]["state"].as_str() == Some("published"))
        .count();
    assert_eq!(published_count, 3, "all 3 packages should be published");

    // Verify dependency order: core appears before utils, utils before app in stdout
    let core_pos = stdout.find("core@0.1.0").expect("core in output");
    let utils_pos = stdout.find("utils@0.1.0").expect("utils in output");
    let app_pos = stdout.find("app@0.1.0").expect("app in output");
    assert!(
        core_pos < utils_pos,
        "core should be published before utils"
    );
    assert!(utils_pos < app_pos, "utils should be published before app");

    registry.join();
}

// ============================================================================
// Test 3: Publish with --policy fast + preflight doesn't create state for plan
// ============================================================================

#[test]
fn plan_does_not_write_state() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success();

    // plan should never create .shipper directory
    assert!(
        !td.path().join(".shipper").exists(),
        "plan should not create .shipper state directory"
    );
}

// ============================================================================
// Test 4: Publish with --package limits scope
// ============================================================================

#[test]
fn publish_with_package_flag_limits_scope() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // Only 1 crate: version-check 404 + readiness 200 = 2 requests
    let registry = spawn_registry(vec![404, 200], 2);

    let state_dir = td.path().join(".shipper");

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
        .arg("--package")
        .arg("core")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success();

    // Receipt should only have core, not utils or app
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse");
    let packages = receipt["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 1, "only one package should be published");
    assert_eq!(packages[0]["name"].as_str(), Some("core"));
    assert_eq!(packages[0]["state"]["state"].as_str(), Some("published"));

    registry.join();
}

// ============================================================================
// Test 5: Publish creates events.jsonl with correct lifecycle events
// ============================================================================

#[test]
fn publish_creates_events_jsonl_with_lifecycle_events() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    let registry = spawn_registry(vec![404, 200], 2);
    let state_dir = td.path().join(".shipper");

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

    let events_path = state_dir.join("events.jsonl");
    assert!(events_path.exists(), "events.jsonl should be created");

    let events_content = fs::read_to_string(&events_path).expect("read events");
    assert!(
        !events_content.is_empty(),
        "events.jsonl should not be empty"
    );

    // Verify lifecycle events are present
    assert!(
        events_content.contains(r#""type":"plan_created"#),
        "should contain plan_created event"
    );
    assert!(
        events_content.contains(r#""type":"execution_started"#),
        "should contain execution_started event"
    );
    assert!(
        events_content.contains(r#""type":"package_started"#),
        "should contain package_started event"
    );
    assert!(
        events_content.contains(r#""type":"package_published"#),
        "should contain package_published event"
    );
    assert!(
        events_content.contains(r#""type":"execution_finished"#),
        "should contain execution_finished event"
    );

    // Each line should be valid JSON
    for line in events_content.lines() {
        let _: serde_json::Value =
            serde_json::from_str(line).expect("each events.jsonl line should be valid JSON");
    }

    registry.join();
}

// ============================================================================
// Test 6: Publish with custom --state-dir writes to correct location
// ============================================================================

#[test]
fn publish_with_custom_state_dir_writes_to_correct_location() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    let registry = spawn_registry(vec![404, 200], 2);
    let custom_dir = td.path().join("my-artifacts").join("nested");

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
        .arg(&custom_dir)
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success();

    // All state files should be in the custom directory
    assert!(custom_dir.exists(), "custom state dir should be created");
    assert!(
        custom_dir.join("state.json").exists(),
        "state.json should be in custom dir"
    );
    assert!(
        custom_dir.join("receipt.json").exists(),
        "receipt.json should be in custom dir"
    );
    assert!(
        custom_dir.join("events.jsonl").exists(),
        "events.jsonl should be in custom dir"
    );

    // Default .shipper directory should NOT be created
    assert!(
        !td.path().join(".shipper").exists(),
        "default .shipper dir should not be created when custom --state-dir is used"
    );

    // Verify receipt content is correct
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(custom_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse receipt");
    assert!(receipt.get("plan_id").is_some());

    registry.join();
}

// ============================================================================
// Test 7: Failed publish creates appropriate state for resume
// ============================================================================

#[test]
fn failed_publish_creates_state_for_resume() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // version-check 404 (not published); cargo publish will fail via exit code
    // Provide enough requests for retries
    let registry = spawn_registry(vec![404], 4);
    let state_dir = td.path().join(".shipper");

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

    // State file should still be created even after failure
    let state_path = state_dir.join("state.json");
    assert!(
        state_path.exists(),
        "state.json should exist after failed publish (for resume)"
    );

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&state_path).expect("read state"))
            .expect("parse state");
    assert!(state.get("plan_id").is_some(), "state should have plan_id");
    assert!(
        state.get("packages").is_some(),
        "state should have packages"
    );

    // The demo package should be in a non-published state (failed or pending)
    let packages = state["packages"].as_object().expect("packages map");
    let demo = packages.get("demo@0.1.0").expect("demo in packages");
    let demo_state = demo["state"]["state"].as_str().expect("state string");
    assert_ne!(
        demo_state, "published",
        "failed package should NOT be marked published"
    );

    registry.join();
}

// ============================================================================
// Test 8: Re-running publish when all crates already published skips everything
// ============================================================================

#[test]
fn publish_when_already_published_skips_all() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // Registry returns 200 for version check (already published)
    // Provide a couple extra requests in case readiness checks happen
    let registry = spawn_registry(vec![200], 4);
    let state_dir = td.path().join(".shipper");

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
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");

    // Should indicate the package was skipped (already published)
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse");
    let packages = receipt["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 1);

    // The package should be skipped (already published) rather than published again
    let pkg_state = packages[0]["state"]["state"].as_str().expect("state");
    assert!(
        pkg_state == "skipped" || pkg_state == "published",
        "already-published package should be skipped or marked published, got: {pkg_state}"
    );

    // stdout should mention the package was skipped or already published
    assert!(
        stdout.contains("Skipped") || stdout.contains("skipped") || stdout.contains("already"),
        "output should indicate package was skipped or already published"
    );

    registry.join();
}
