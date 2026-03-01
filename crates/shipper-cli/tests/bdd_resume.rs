//! BDD (Behavior-Driven Development) tests for the shipper resume workflow.
//!
//! These tests correspond to the scenarios in `features/resume_workflow.feature`
//! and verify that `shipper resume` behaves correctly when:
//!
//! 1. Resuming from an interrupted publish continues where it left off
//! 2. Resuming with no state file fails with a clear error
//! 3. Resuming with a fully completed state reports success
//! 4. Resuming with a plan_id mismatch rejects and shows an error
//! 5. Resuming from a specific package via `--resume-from`
//! 6. The state file is updated atomically after each package

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
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
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

fn write_state_json(state_dir: &Path, json: &str) {
    fs::create_dir_all(state_dir).expect("mkdir state dir");
    fs::write(state_dir.join("state.json"), json).expect("write state.json");
}

/// Common shipper arguments that disable delays and set fast timeouts.
fn fast_resume_args(cmd: &mut Command, manifest: &Path, api_base: &str, state_dir: &Path) {
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

// ============================================================================
// Feature: Resume workflow
// ============================================================================

// ----------------------------------------------------------------------------
// Scenario 1: Resume continues from where publish was interrupted
// (feature line 8)
// ----------------------------------------------------------------------------
mod resume_continues_interrupted {
    use super::*;

    /// Given an existing state that marks "core@0.1.0" as Published and
    ///   "app@0.1.0" as Pending,
    /// And the registry returns published for core and not-found for app,
    /// And cargo publish succeeds for app,
    /// When I run "shipper resume",
    /// Then the exit code is 0 and the receipt shows app as Published.
    #[test]
    #[serial]
    fn given_interrupted_state_when_resume_then_continues_where_left_off() {
        let td = tempdir().expect("tempdir");
        create_two_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // First run: publish so state.json is generated with the real plan_id.
        // Make cargo publish fail so we get a state with a failed package.
        // core version-check 200 (already published → skip), app version-check 404,
        // app cargo fails, post-failure 404, final-chance 404 → 5 requests.
        // Resume: app version-check 404, cargo succeeds, readiness 200 → 2 requests.
        let registry = spawn_registry(vec![200, 404, 404, 404, 404, 200], 7);

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

        // Verify state: core skipped/published, app failed
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
        )
        .expect("parse state");
        let pkgs = state["packages"].as_object().expect("packages");
        let app_state = pkgs["app@0.1.0"]["state"]["state"]
            .as_str()
            .expect("app state");
        assert_eq!(app_state, "failed", "app should be failed before resume");

        // When: resume with cargo publish succeeding
        let mut cmd = shipper_cmd();
        fast_resume_args(
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
            Some("published")
        );

        registry.join();
    }
}

// ----------------------------------------------------------------------------
// Scenario 2: Resume with no state file fails with clear error
// (feature line 50)
// ----------------------------------------------------------------------------
mod resume_no_state {
    use super::*;

    /// Given no state file exists,
    /// When I run "shipper resume",
    /// Then the exit code is non-zero and the error mentions missing state.
    #[test]
    fn given_no_state_file_when_resume_then_fails_with_clear_error() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("custom-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");

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

    /// Given an empty state directory,
    /// When I run "shipper resume",
    /// Then it reports no state found.
    #[test]
    fn given_empty_state_dir_when_resume_then_reports_no_state_found() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("empty-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");

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

    /// Given a corrupted state file,
    /// When I run "shipper resume",
    /// Then it reports a parse error.
    #[test]
    fn given_corrupted_state_file_when_resume_then_reports_parse_error() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("corrupt-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("state.json"), "NOT VALID JSON {{{").expect("write corrupt state");

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            .assert()
            .failure()
            .stderr(contains("failed to parse state JSON"));
    }

    /// Given --state-dir points to a path that does not exist,
    /// When I run "shipper resume",
    /// Then it fails because no state can be found.
    #[test]
    fn given_nonexistent_state_dir_when_resume_then_fails_appropriately() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("does-not-exist");

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
}

// ----------------------------------------------------------------------------
// Scenario 3: Resume with completed state reports success
// (feature line 56)
// ----------------------------------------------------------------------------
mod resume_completed_state {
    use super::*;

    /// Given an existing state file marks all packages as Published,
    /// When I run "shipper resume",
    /// Then the exit code is 0, cargo publish was not invoked,
    /// and the output reports packages as already complete.
    #[test]
    #[serial]
    fn given_all_published_state_when_resume_then_succeeds_without_publish() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // First publish successfully to create state with correct plan_id.
        // version-check 404, readiness 200 → 2 requests.
        // Resume: all Published → 0 requests but keep server alive.
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

        // Verify state shows published
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
        )
        .expect("parse state");
        assert_eq!(
            state["packages"]["demo@0.1.0"]["state"]["state"].as_str(),
            Some("published")
        );

        // When: resume with completed state
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

        // Then: engine reports "already complete" for published packages
        let stderr = String::from_utf8(output).expect("utf8");
        assert!(
            stderr.contains("already complete"),
            "should report already complete, got stderr: {stderr}"
        );

        registry.join();
    }
}

// ----------------------------------------------------------------------------
// Scenario 4: Resume with plan_id mismatch rejects and shows error
// (feature line 26)
// ----------------------------------------------------------------------------
mod resume_plan_id_mismatch {
    use super::*;

    /// Given an existing state file with plan_id "abc123",
    /// And the current workspace generates a different plan_id,
    /// When I run "shipper resume",
    /// Then the exit code is non-zero and the error mentions plan_id.
    #[test]
    fn given_mismatched_plan_id_when_resume_then_rejects_with_error() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join(".shipper");

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

        // When: resume with mismatched plan_id
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

    /// Given an existing state file with a wrong plan_id,
    /// When I run "shipper resume" with "--force-resume",
    /// Then the plan_id mismatch is bypassed.
    #[test]
    #[serial]
    fn given_mismatched_plan_id_when_force_resume_then_bypasses_mismatch() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // Use a real publish to get valid state, then tamper with plan_id.
        // version-check 404, readiness 200 → 2 for initial.
        // force-resume: version-check 404, readiness 200 → 2 more.
        let registry = spawn_registry(vec![404, 200, 404, 200], 5);

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

        // Tamper with plan_id in the state file
        let state_path = state_dir.join("state.json");
        let raw = fs::read_to_string(&state_path).expect("read state");
        let mut state: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        state["plan_id"] = serde_json::Value::String("tampered-plan-id".to_string());
        fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).expect("write");

        // When: force-resume bypasses mismatch
        let mut cmd = shipper_cmd();
        fast_resume_args(
            &mut cmd,
            &td.path().join("Cargo.toml"),
            &registry.base_url,
            &state_dir,
        );
        cmd.arg("--force-resume")
            .arg("resume")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        registry.join();
    }
}

// ----------------------------------------------------------------------------
// Scenario 5: Resume from a specific package via --resume-from
// (feature line 41)
// ----------------------------------------------------------------------------
mod resume_from_specific_package {
    use super::*;

    /// Given an existing state that marks "core@0.1.0" as Failed and
    ///   "app@0.1.0" as Pending,
    /// When I run "shipper resume" with "--resume-from core",
    /// Then cargo publish is invoked for core and the exit code is 0.
    #[test]
    #[serial]
    fn given_failed_core_when_resume_from_core_then_publishes_core() {
        let td = tempdir().expect("tempdir");
        create_two_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // Initial publish: all fail.
        // core version-check 404, cargo fails, post-failure 404, final-chance 404 → 3 requests.
        // app version-check 404, cargo fails, post-failure 404, final-chance 404 → 3 requests.
        // Resume with --resume-from core:
        //   core version-check 404, cargo succeeds, readiness 200 → 2 requests.
        //   app version-check 404, cargo succeeds, readiness 200 → 2 requests.
        let registry = spawn_registry(vec![404, 404, 404, 404, 404, 404, 404, 200, 404, 200], 11);

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

        // Verify core is failed in state
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
        )
        .expect("parse state");
        let core_state = state["packages"]["core@0.1.0"]["state"]["state"]
            .as_str()
            .expect("core state");
        assert_eq!(core_state, "failed", "core should be failed before resume");

        // When: resume from core specifically
        let mut cmd = shipper_cmd();
        fast_resume_args(
            &mut cmd,
            &td.path().join("Cargo.toml"),
            &registry.base_url,
            &state_dir,
        );
        cmd.arg("--resume-from")
            .arg("core")
            .arg("resume")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then: final state shows core published
        let final_state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
        )
        .expect("parse state");
        let final_core = final_state["packages"]["core@0.1.0"]["state"]["state"]
            .as_str()
            .expect("core state");
        assert_eq!(
            final_core, "published",
            "core should be published after resume-from, got: {final_core}"
        );

        registry.join();
    }
}

// ----------------------------------------------------------------------------
// Scenario 6: State file is updated atomically after each package
// (feature line 73)
// ----------------------------------------------------------------------------
mod state_updated_atomically {
    use super::*;

    /// Given an existing state that marks "core@0.1.0" as Pending,
    /// And cargo publish succeeds for core,
    /// When I run "shipper resume",
    /// Then the state file is valid JSON after completion and reflects the
    ///   updated package states.
    #[test]
    #[serial]
    fn given_pending_state_when_resume_then_state_file_is_valid_json() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // Publish fails first: version-check 404, cargo fails,
        // post-failure 404, final-chance 404 → 3 requests.
        // Resume: version-check 404, cargo succeeds, readiness 200 → 2 requests.
        let registry = spawn_registry(vec![404, 404, 404, 404, 200], 6);

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

        // When: resume succeeds
        let mut cmd = shipper_cmd();
        fast_resume_args(
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

        // Then: state file is valid JSON
        let state_raw = fs::read_to_string(state_dir.join("state.json")).expect("read state.json");
        let state: serde_json::Value =
            serde_json::from_str(&state_raw).expect("state.json should be valid JSON");

        // Verify structural integrity
        assert!(state["state_version"].is_string(), "state_version present");
        assert!(state["plan_id"].is_string(), "plan_id present");
        assert!(state["packages"].is_object(), "packages is an object");

        // All packages should reflect a terminal state
        let pkgs = state["packages"].as_object().expect("packages");
        for (key, pkg) in pkgs {
            let pkg_state = pkg["state"]["state"].as_str().unwrap_or("unknown");
            assert!(
                matches!(pkg_state, "published" | "skipped"),
                "package {key} should be in terminal state, got: {pkg_state}"
            );
        }

        registry.join();
    }

    /// State file reflects updated package states after resume completes for
    /// a two-crate workspace.
    #[test]
    #[serial]
    fn given_two_crate_workspace_when_resume_then_state_reflects_all_updates() {
        let td = tempdir().expect("tempdir");
        create_two_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
        let state_dir = td.path().join(".shipper");

        // Publish: core 200 (skip), app 404 cargo-fail 404 404 → 4 requests.
        // Resume: app 404, cargo ok, 200 → 2 requests.
        let registry = spawn_registry(vec![200, 404, 404, 404, 404, 200], 7);

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

        // When: resume
        let mut cmd = shipper_cmd();
        fast_resume_args(
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

        // Then: both packages are in a terminal state
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
        )
        .expect("parse state");

        let pkgs = state["packages"].as_object().expect("packages");
        let core_s = pkgs["core@0.1.0"]["state"]["state"]
            .as_str()
            .expect("core state");
        let app_s = pkgs["app@0.1.0"]["state"]["state"]
            .as_str()
            .expect("app state");
        assert!(
            core_s == "skipped" || core_s == "published",
            "core in terminal state, got: {core_s}"
        );
        assert_eq!(app_s, "published", "app should be published, got: {app_s}");

        registry.join();
    }
}
