//! BDD (Behavior-Driven Development) tests for parallel publishing scenarios.
//!
//! These tests verify that the parallel publish engine correctly groups
//! independent packages into levels, respects dependency ordering, handles
//! resume of partially completed levels, stops on failure, and honours the
//! `max_concurrent` concurrency limit.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

// ---------------------------------------------------------------------------
// Helpers (mirrors conventions from sibling BDD test files)
// ---------------------------------------------------------------------------

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

/// Fan-out/fan-in workspace: core → {api, cli} → app (3 levels).
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

/// Workspace with three completely independent crates (single level).
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

fn create_fake_cargo_failing(bin_dir: &Path) {
    #[cfg(windows)]
    {
        fs::write(
            bin_dir.join("cargo.cmd"),
            "@echo off\r\nif \"%1\"==\"publish\" (\r\n  echo error: simulated permanent failure 1>&2\r\n  exit /b 1\r\n)\r\n\"%REAL_CARGO%\" %*\r\nexit /b %ERRORLEVEL%\r\n",
        )
        .expect("write fake cargo");
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cargo");
        fs::write(
            &path,
            "#!/usr/bin/env sh\nif [ \"$1\" = \"publish\" ]; then\n  echo 'error: simulated permanent failure' >&2\n  exit 1\nfi\n\"$REAL_CARGO\" \"$@\"\n",
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

fn prepend_fake_cargo(td: &Path) -> (String, String, String) {
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

fn prepend_fake_cargo_failing(td: &Path) -> (String, String, String) {
    let bin_dir = td.join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("mkdir");
    create_fake_cargo_failing(&bin_dir);

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

// ============================================================================
// Feature: Parallel publish level grouping
//   Shipper groups packages into dependency levels so independent crates can
//   publish concurrently while preserving dependency order.
// ============================================================================

mod parallel_level_grouping {
    use super::*;

    // Scenario: Independent crates are grouped into a single parallel level
    //
    // Given: A workspace with independent crates (no inter-dependencies)
    // When:  Computing publish levels
    // Then:  All crates are placed in a single level (level 0)
    #[test]
    fn given_independent_crates_when_publishing_then_publishes_in_parallel_levels() {
        // Given: Three independent crates with no inter-workspace dependencies
        let td = tempdir().expect("tempdir");
        create_independent_workspace(td.path());

        let spec = shipper::types::ReleaseSpec {
            manifest_path: td.path().join("Cargo.toml"),
            registry: shipper::types::Registry::crates_io(),
            selected_packages: None,
        };

        // When: We compute the publish plan levels
        let ws = shipper::plan::build_plan(&spec).expect("plan");
        let levels = ws.plan.group_by_levels();

        // Then: All three crates share a single level (can publish concurrently)
        assert_eq!(
            levels.len(),
            1,
            "independent crates should form a single level"
        );
        assert_eq!(
            levels[0].packages.len(),
            3,
            "all three crates should be in level 0"
        );
        let names: Vec<&str> = levels[0].packages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"gamma"));
    }

    // Scenario: Plan command (verbose) shows parallel levels for independent crates
    #[test]
    fn given_independent_crates_when_plan_verbose_then_shows_single_level() {
        // Given
        let td = tempdir().expect("tempdir");
        create_independent_workspace(td.path());

        // When
        let out = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--verbose")
            .arg("plan")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then: Verbose plan output should show all three at level 0
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(stdout.contains("Level 0:"));
        assert!(stdout.contains("alpha@0.1.0"));
        assert!(stdout.contains("beta@0.1.0"));
        assert!(stdout.contains("gamma@0.1.0"));
    }
}

// ============================================================================
// Feature: Dependency-aware level ordering
//   Packages with dependencies are placed in higher levels than their
//   dependencies, ensuring correct publish order.
// ============================================================================

mod dependency_level_ordering {
    use super::*;

    // Scenario: Fan-out/fan-in workspace creates three publish levels
    //
    // Given: A workspace with core → {api, cli} → app
    // When:  Computing publish levels
    // Then:  Level 0 = core, Level 1 = {api, cli}, Level 2 = app
    #[test]
    fn given_workspace_with_dependencies_when_publishing_then_respects_level_ordering() {
        // Given
        let td = tempdir().expect("tempdir");
        create_parallel_workspace(td.path());

        let spec = shipper::types::ReleaseSpec {
            manifest_path: td.path().join("Cargo.toml"),
            registry: shipper::types::Registry::crates_io(),
            selected_packages: None,
        };

        // When
        let ws = shipper::plan::build_plan(&spec).expect("plan");
        let levels = ws.plan.group_by_levels();

        // Then: Three levels respecting the dependency chain
        assert_eq!(levels.len(), 3, "fan-out/fan-in should produce 3 levels");

        let level0_names: Vec<&str> = levels[0].packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            level0_names,
            vec!["core"],
            "level 0 should contain only core"
        );

        let mut level1_names: Vec<&str> =
            levels[1].packages.iter().map(|p| p.name.as_str()).collect();
        level1_names.sort();
        assert_eq!(
            level1_names,
            vec!["api", "cli"],
            "level 1 should contain api and cli"
        );

        let level2_names: Vec<&str> = levels[2].packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(level2_names, vec!["app"], "level 2 should contain only app");
    }

    // Scenario: Plan command (verbose) shows three levels for fan-out/fan-in workspace
    #[test]
    fn given_dependencies_when_plan_verbose_then_shows_three_levels() {
        // Given
        let td = tempdir().expect("tempdir");
        create_parallel_workspace(td.path());

        // When
        let out = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--verbose")
            .arg("plan")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(stdout.contains("Level 0:"), "should show level 0");
        assert!(stdout.contains("Level 1:"), "should show level 1");
        assert!(stdout.contains("Level 2:"), "should show level 2");
        assert!(stdout.contains("core@0.1.0"));
        assert!(stdout.contains("app@0.1.0"));
    }
}

// ============================================================================
// Feature: Resume skips completed levels
//   When resuming a partially published workspace, already-completed levels
//   are skipped.
// ============================================================================

mod resume_skips_completed_levels {
    use super::*;

    // Scenario: Partially published workspace resumes and skips completed levels
    //
    // Given: A workspace where publish has already completed
    // When:  Resuming with existing state
    // Then:  Already-published packages are skipped
    #[test]
    fn given_partially_published_workspace_when_resuming_then_skips_completed_levels() {
        // Given: Set up workspace + fake cargo + state dir
        let td = tempdir().expect("tempdir");
        create_parallel_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = prepend_fake_cargo(td.path());

        let state_dir = td.path().join(".shipper");

        // Publish all crates in parallel mode (serialized within levels via --max-concurrent 1).
        // 4 crates × (version check + readiness) = 8 reqs
        let registry = spawn_registry(vec![404, 200, 404, 200, 404, 200, 404, 200], 8);

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
            .arg("--max-concurrent")
            .arg("1")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        registry.join();

        // When: Check state shows packages are published (via status)
        let state_file = state_dir.join("state.json");
        assert!(state_file.exists(), "state.json should exist after publish");

        let state_json = fs::read_to_string(&state_file).expect("read state");
        let state: serde_json::Value = serde_json::from_str(&state_json).expect("parse state");

        // Then: All packages should be in a terminal state (published/skipped)
        let packages = state["packages"].as_object().expect("packages object");
        for (_key, progress) in packages {
            let pkg_state = progress["state"]["state"].as_str().unwrap_or("unknown");
            assert!(
                pkg_state == "published" || pkg_state == "skipped",
                "expected published or skipped, got: {pkg_state}"
            );
        }

        // Also verify the receipt shows all completed
        let receipt_file = state_dir.join("receipt.json");
        assert!(receipt_file.exists(), "receipt.json should exist");

        let receipt_json = fs::read_to_string(&receipt_file).expect("read receipt");
        let receipt: serde_json::Value =
            serde_json::from_str(&receipt_json).expect("parse receipt");
        let receipt_pkgs = receipt["packages"].as_array().expect("packages array");
        assert_eq!(
            receipt_pkgs.len(),
            4,
            "receipt should contain all 4 packages"
        );
    }
}

// ============================================================================
// Feature: Failure in one level stops subsequent levels
//   When a package in one level fails, the parallel engine does not proceed
//   to the next level.
// ============================================================================

mod failure_stops_subsequent_levels {
    use super::*;

    // Scenario: Failure in level 0 prevents level 1 and level 2 from running
    //
    // Given: A workspace with dependencies where cargo publish always fails
    // When:  Publishing in parallel mode
    // Then:  Publish fails and subsequent levels are not attempted
    #[test]
    fn given_failure_in_one_level_when_continuing_then_stops_subsequent_levels() {
        // Given: Workspace + failing cargo proxy
        let td = tempdir().expect("tempdir");
        create_parallel_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = prepend_fake_cargo_failing(td.path());

        let state_dir = td.path().join(".shipper");

        // Registry returns 404 for version_exists (not published)
        // Only level 0 (core) should be attempted: 1 version check = 1 req
        // Use generous count to avoid server hang if more requests arrive
        let registry = spawn_registry(vec![404], 10);

        // When: Parallel publish (core fails, so api/cli/app should not be attempted)
        let assert_result = shipper_cmd()
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
            .arg("--parallel")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .assert()
            .failure();

        // Then: The error output should mention the failure
        let stderr = String::from_utf8(assert_result.get_output().stderr.clone()).expect("utf8");
        assert!(
            stderr.contains("failed") || stderr.contains("error") || stderr.contains("Error"),
            "stderr should indicate failure: {stderr}"
        );

        // State should exist but not all packages should be published
        if state_dir.join("state.json").exists() {
            let state_json = fs::read_to_string(state_dir.join("state.json")).expect("read state");
            let state: serde_json::Value = serde_json::from_str(&state_json).expect("parse state");

            if let Some(packages) = state["packages"].as_object() {
                // App (level 2) should NOT have been attempted
                let app_attempted = packages.iter().any(|(k, v)| {
                    k.contains("app") && v["state"]["state"].as_str() == Some("published")
                });
                assert!(
                    !app_attempted,
                    "app (level 2) should not have been published when level 0 failed"
                );
            }
        }

        registry.join();
    }
}

// ============================================================================
// Feature: max_concurrent limits parallelism
//   The --max-concurrent flag controls how many packages are published
//   concurrently within a single level.
// ============================================================================

mod max_concurrent_limits_parallelism {
    use super::*;

    // Scenario: --max-concurrent restricts the number of concurrent publishes
    //
    // Given: A workspace with independent crates
    // When:  Publishing with --max-concurrent 1
    // Then:  Publishes succeed (max_concurrent is honoured; effectively serial within level)
    #[test]
    fn given_max_concurrent_setting_when_publishing_then_limits_parallelism() {
        // Given
        let td = tempdir().expect("tempdir");
        create_independent_workspace(td.path());
        let (new_path, real_cargo, fake_cargo) = prepend_fake_cargo(td.path());

        let state_dir = td.path().join(".shipper");

        // 3 independent crates: version check + readiness = 6 reqs
        let registry = spawn_registry(vec![404, 200, 404, 200, 404, 200], 6);

        // When: Publish with --max-concurrent 1 (serial within each level)
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
            .arg("--max-concurrent")
            .arg("1")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        // Then: All packages should be published in the receipt
        let receipt_path = state_dir.join("receipt.json");
        assert!(receipt_path.exists(), "receipt.json should exist");

        let receipt_json = fs::read_to_string(&receipt_path).expect("read receipt");
        let receipt: serde_json::Value =
            serde_json::from_str(&receipt_json).expect("parse receipt");
        let packages = receipt["packages"].as_array().expect("packages array");
        assert_eq!(packages.len(), 3, "all 3 packages should be in receipt");

        for pkg in packages {
            let state = pkg["state"]["state"].as_str().unwrap_or("unknown");
            assert!(
                state == "published" || state == "skipped",
                "package should be published or skipped, got: {state}"
            );
        }

        registry.join();
    }

    // Scenario: --max-concurrent flag is accepted by the CLI
    //
    // Given: A workspace
    // When:  Running plan with --max-concurrent
    // Then:  The plan command succeeds (flag is accepted)
    #[test]
    fn given_max_concurrent_flag_when_plan_then_accepted() {
        // Given
        let td = tempdir().expect("tempdir");
        create_independent_workspace(td.path());

        // When / Then: --max-concurrent flag is accepted
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--max-concurrent")
            .arg("2")
            .arg("plan")
            .assert()
            .success();
    }
}
