//! BDD (Behavior-Driven Development) tests for advanced publish scenarios.
//!
//! These tests cover edge cases around registry errors (401, 429), retry
//! behaviour on cargo-publish failures, plan-as-dry-run previews, skipping
//! already-published crates, and partial multi-crate failures.
//!
//! NOTE: On Windows, run with `--test-threads=1` if any tests fail with
//! "failed to send request to registry" due to concurrent tiny_http
//! servers exhausting network resources.

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

/// Fake cargo that succeeds on the first `publish` invocation then fails on all
/// subsequent ones. Uses a flag file (`SHIPPER_FAIL_FLAG` env var) to track
/// whether the first call has already happened.
fn create_succeed_then_fail_cargo(bin_dir: &Path) {
    #[cfg(windows)]
    {
        fs::write(
            bin_dir.join("cargo.cmd"),
            "@echo off\r\n\
             if not \"%1\"==\"publish\" goto :fallback\r\n\
             if exist \"%SHIPPER_FAIL_FLAG%\" goto :fail\r\n\
             echo done > \"%SHIPPER_FAIL_FLAG%\"\r\n\
             exit /b 0\r\n\
             :fail\r\n\
             echo cargo publish failed 1>&2\r\n\
             exit /b 1\r\n\
             :fallback\r\n\
             \"%REAL_CARGO%\" %*\r\n\
             exit /b %ERRORLEVEL%\r\n",
        )
        .expect("write fake cargo");
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cargo");
        fs::write(
            &path,
            "#!/usr/bin/env sh\n\
             if [ \"$1\" = \"publish\" ]; then\n\
             if [ -f \"$SHIPPER_FAIL_FLAG\" ]; then\n\
             echo 'cargo publish failed' >&2\n\
             exit 1\n\
             fi\n\
             touch \"$SHIPPER_FAIL_FLAG\"\n\
             exit 0\n\
             fi\n\
             \"$REAL_CARGO\" \"$@\"\n",
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
    #[allow(dead_code)]
    handle: thread::JoinHandle<()>,
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
    // Give the server thread a moment to enter its accept loop
    thread::sleep(Duration::from_millis(50));
    TestRegistry { base_url, handle }
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

fn setup_fake_cargo(td: &Path) -> (String, String, String) {
    let bin_dir = td.join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("mkdir");
    create_fake_cargo_proxy(&bin_dir);
    fake_cargo_env(&bin_dir)
}

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

// ============================================================================
// Feature: Registry Error Handling During Publish
// ============================================================================

mod registry_errors {
    use super::*;

    // Scenario: Registry returns 401 Unauthorized during version check
    //
    // Given: A single-crate workspace and a mock registry returning 401
    // When:  Running `publish`
    // Then:  Publish fails with an error mentioning the unexpected status
    #[test]
    fn given_registry_returns_401_when_publish_then_fails_with_auth_error() {
        // Given
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

        // Registry returns 401 on the version-existence check
        let registry = spawn_registry(vec![401], 1);
        let state_dir = td.path().join(".shipper");

        // When / Then
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
            .failure()
            .stderr(contains("unexpected status").or(contains("401")));

        drop(registry);
    }

    // Scenario: Registry returns 429 Too Many Requests during version check
    //
    // Given: A single-crate workspace and a mock registry returning 429
    // When:  Running `publish`
    // Then:  Publish fails with an error mentioning the unexpected status
    #[test]
    fn given_registry_returns_429_when_publish_then_fails_with_rate_limit_error() {
        // Given
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

        // Registry returns 429 on the version-existence check
        let registry = spawn_registry(vec![429], 1);
        let state_dir = td.path().join(".shipper");

        // When / Then
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
            .failure()
            .stderr(contains("unexpected status").or(contains("429")));

        drop(registry);
    }
}

// ============================================================================
// Feature: Retry Behaviour on Cargo Publish Failures
// ============================================================================

mod retry_on_failure {
    use super::*;

    // Scenario: Cargo publish fails repeatedly and exhausts retries
    //
    // Given: A single-crate workspace, a fake cargo that always exits 1,
    //        and a registry that always returns 404 (not published)
    // When:  Running `publish` with --max-attempts 2 --base-delay 0ms
    // Then:  Publish eventually fails after retrying, and state is saved
    #[test]
    fn given_cargo_fails_when_publish_with_retries_then_exhausts_attempts_and_fails() {
        // Given
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

        // Registry always returns 404 (version never appears):
        // - 1 initial version check
        // - 1 check after each failed attempt (2 attempts)
        // - 1 final check after loop
        let registry = spawn_registry(vec![404], 4);
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
            .arg("2")
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

        // Then: state.json should exist with the failed package status
        let state_path = state_dir.join("state.json");
        assert!(
            state_path.exists(),
            "state.json should be created even after publish failure"
        );

        let state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&state_path).expect("read state"))
                .expect("parse state");
        let packages = state["packages"].as_object().expect("packages map");
        let demo = packages.get("demo@0.1.0").expect("demo in state");
        let demo_state = demo["state"]["state"].as_str().expect("state string");
        assert_ne!(
            demo_state, "published",
            "failed package should not be marked published"
        );

        drop(registry);
    }
}

// ============================================================================
// Feature: Plan as Dry-Run Preview
// ============================================================================

mod plan_as_dry_run {
    use super::*;

    // Scenario: `plan` shows the publish order but does not publish
    //
    // Given: A multi-crate workspace (core → utils → app)
    // When:  Running `plan`
    // Then:  All crates listed in dependency order, no state directory created
    #[test]
    fn given_workspace_when_plan_then_shows_order_without_side_effects() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // When
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

        // Then: all packages listed
        assert!(stdout.contains("core@0.1.0"), "core should appear in plan");
        assert!(
            stdout.contains("utils@0.1.0"),
            "utils should appear in plan"
        );
        assert!(stdout.contains("app@0.1.0"), "app should appear in plan");

        // Then: dependency order is respected (core before utils before app)
        let core_pos = stdout.find("core@0.1.0").expect("core in output");
        let utils_pos = stdout.find("utils@0.1.0").expect("utils in output");
        let app_pos = stdout.find("app@0.1.0").expect("app in output");
        assert!(
            core_pos < utils_pos,
            "core should appear before utils in plan"
        );
        assert!(
            utils_pos < app_pos,
            "utils should appear before app in plan"
        );

        // Then: no state artifacts created (plan is read-only)
        assert!(
            !td.path().join(".shipper").exists(),
            "plan should not create .shipper state directory"
        );
    }

    // Scenario: `plan` with --package shows only the targeted package
    //
    // Given: A multi-crate workspace
    // When:  Running `plan --package core`
    // Then:  Only core is listed; no other packages appear
    #[test]
    fn given_workspace_when_plan_with_package_then_shows_only_that_package() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // When
        let output = shipper_cmd()
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

        let stdout = String::from_utf8(output).expect("utf8");

        // Then
        assert!(stdout.contains("core@0.1.0"), "core should be in plan");
        assert!(
            !stdout.contains("utils@0.1.0"),
            "utils should NOT be in plan"
        );
        assert!(!stdout.contains("app@0.1.0"), "app should NOT be in plan");
    }
}

// ============================================================================
// Feature: Skipping Already-Published Crates
// ============================================================================

mod already_published_skip {
    use super::*;

    // Scenario: Crate already on registry is skipped during publish
    //
    // Given: A single-crate workspace and a registry that reports 200 for
    //        the version check (crate already published)
    // When:  Running `publish`
    // Then:  Publish succeeds, receipt shows the package as skipped
    #[test]
    fn given_already_published_crate_when_publish_then_skipped_in_receipt() {
        // Given
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

        // Registry returns 200 for version check (already published)
        let registry = spawn_registry(vec![200], 1);
        let state_dir = td.path().join(".shipper");

        // When
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

        // Then: receipt shows the package as skipped (or published for already-present)
        let receipt: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
        )
        .expect("parse receipt");
        let packages = receipt["packages"].as_array().expect("packages");
        assert_eq!(packages.len(), 1);

        let pkg_state = packages[0]["state"]["state"].as_str().expect("state");
        assert!(
            pkg_state == "skipped" || pkg_state == "published",
            "already-published package should be skipped or marked published, got: {pkg_state}"
        );

        // stdout should indicate skipping
        assert!(
            stdout.contains("Skipped") || stdout.contains("skipped") || stdout.contains("already"),
            "output should indicate skipping or already published"
        );

        drop(registry);
    }
}

// ============================================================================
// Feature: Multi-Crate Partial Publish Failure
// ============================================================================

mod multi_crate_partial_failure {
    use super::*;

    // Scenario: Middle crate fails, later crates are not attempted, state saved
    //
    // Given: A 3-crate workspace (core → utils → app) where cargo publish
    //        succeeds for the first package but fails for subsequent ones
    // When:  Running `publish` with --max-attempts 1
    // Then:  Publish fails; state.json shows core as published, utils as
    //        failed, and app still pending (never attempted)
    #[test]
    fn given_middle_crate_fails_when_publish_then_later_crates_not_attempted() {
        // Given
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        create_succeed_then_fail_cargo(&bin_dir);
        let (new_path, real_cargo, fake_cargo) = fake_cargo_env(&bin_dir);

        let fail_flag = td.path().join("publish-fail-flag");

        // Registry responses in order:
        //   1. version check for core → 404 (not published)
        //   2. readiness check for core → 200 (visible)
        //   3. version check for utils → 404 (not published)
        //   4. version check after utils failure → 404 (not visible)
        //   5. final version check for utils → 404
        // Extra buffer for any additional readiness/verification requests.
        let registry = spawn_registry(vec![404, 200, 404, 404, 404], 5);
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
            .arg("--base-delay")
            .arg("0ms")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAIL_FLAG", &fail_flag)
            .assert()
            .failure();

        // Then: state.json should exist with package statuses
        let state_path = state_dir.join("state.json");
        assert!(state_path.exists(), "state.json should exist after failure");

        let state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&state_path).expect("read state"))
                .expect("parse state");
        let packages = state["packages"].as_object().expect("packages map");

        // Core should have been published successfully
        let core = packages.get("core@0.1.0").expect("core in state");
        assert_eq!(
            core["state"]["state"].as_str(),
            Some("published"),
            "core should be published"
        );

        // Utils should have failed
        let utils = packages.get("utils@0.1.0").expect("utils in state");
        let utils_state = utils["state"]["state"].as_str().expect("utils state");
        assert_eq!(utils_state, "failed", "utils should be marked failed");

        // App should still be pending (never started)
        let app = packages.get("app@0.1.0").expect("app in state");
        assert_eq!(
            app["state"]["state"].as_str(),
            Some("pending"),
            "app should still be pending (never attempted)"
        );

        // No receipt should be written on partial failure
        assert!(
            !state_dir.join("receipt.json").exists(),
            "receipt.json should NOT be written on partial failure"
        );

        drop(registry);
    }
}
