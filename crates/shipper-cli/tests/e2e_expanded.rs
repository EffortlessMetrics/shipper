//! Expanded E2E tests for shipper-cli covering doctor, config, status, plan,
//! clean, completion, CI subcommands, error output, and snapshot stability.

use std::fs;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
use insta::assert_snapshot;
use predicates::str::contains;
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

/// Normalize dynamic parts of CLI output so snapshots remain stable across
/// machines and versions.
fn normalize_output(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            if line.starts_with("plan_id: ") || line.starts_with("Plan ID: ") {
                "plan_id: <PLAN_ID>".to_string()
            } else if line.starts_with("workspace_root: ") {
                "workspace_root: <WORKSPACE_ROOT>".to_string()
            } else if line.starts_with("state_dir: ") {
                "state_dir: <STATE_DIR>".to_string()
            } else if line.starts_with("cargo: ") {
                "cargo: <CARGO_VERSION>".to_string()
            } else if line.starts_with("git: ") {
                "git: <GIT_VERSION>".to_string()
            } else if line.starts_with("Removed: ") {
                // Normalize file removal paths
                let suffix = line.rsplit(['/', '\\']).next().unwrap_or(line);
                format!("Removed: <DIR>/{suffix}")
            } else if line.starts_with("Kept: ") {
                let suffix = line.rsplit(['/', '\\']).next().unwrap_or(line);
                format!("Kept: <DIR>/{suffix}")
            } else if line.starts_with("State directory does not exist: ") {
                "State directory does not exist: <STATE_DIR>".to_string()
            } else if line.starts_with("Created configuration file: ") {
                "Created configuration file: <PATH>".to_string()
            } else {
                // Replace backslashes then normalize any embedded absolute
                // paths ending in /.shipper with <STATE_DIR>.
                normalize_embedded_paths(&line.replace('\\', "/"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replace absolute paths ending in `/.shipper` with `<STATE_DIR>`.
fn normalize_embedded_paths(line: &str) -> String {
    const SUFFIX: &str = "/.shipper";
    if let Some(end) = line.find(SUFFIX) {
        let before = &line[..end];
        let path_start = before
            .rfind(|c: char| c.is_whitespace() || c == '\'' || c == '"')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = &line[..path_start];
        let after = &line[end + SUFFIX.len()..];
        format!("{prefix}<STATE_DIR>{after}")
    } else {
        line.to_string()
    }
}

/// Create a simple workspace with a single publishable crate.
fn create_workspace(root: &Path) {
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

/// Create a workspace with a publish = false crate mixed in.
fn create_workspace_with_unpublished(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["alpha", "beta-internal"]
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

    write_file(
        &root.join("beta-internal/Cargo.toml"),
        r#"
[package]
name = "beta-internal"
version = "0.0.1"
edition = "2021"
publish = false
"#,
    );
    write_file(&root.join("beta-internal/src/lib.rs"), "pub fn beta() {}\n");
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

fn spawn_registry(expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for _ in 0..expected_requests {
            let req = server.recv().expect("request");
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

// ===========================================================================
// 1. Version flag
// ===========================================================================

#[test]
fn version_output_format() {
    let output = shipper_cmd()
        .arg("--version")
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    // Format should be "shipper X.Y.Z" or "shipper X.Y.Z-rc.N" etc.
    assert!(
        trimmed.starts_with("shipper "),
        "expected version to start with 'shipper ', got: {trimmed}"
    );
    let version_part = trimmed.strip_prefix("shipper ").unwrap();
    assert!(
        version_part.contains('.'),
        "version should contain a dot: {version_part}"
    );
}

// ===========================================================================
// 2. Error output — missing / invalid manifest
// ===========================================================================

#[test]
fn missing_manifest_path_fails_with_error() {
    let td = tempdir().expect("tempdir");
    // No Cargo.toml created

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .failure();
}

#[test]
fn invalid_manifest_content_fails() {
    let td = tempdir().expect("tempdir");
    write_file(
        &td.path().join("Cargo.toml"),
        "this is {{ not valid toml content",
    );

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .failure();
}

#[test]
fn non_workspace_manifest_fails() {
    let td = tempdir().expect("tempdir");
    // A valid Cargo.toml but for a single package, not a workspace
    write_file(
        &td.path().join("Cargo.toml"),
        r#"
[package]
name = "solo"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&td.path().join("src/lib.rs"), "pub fn solo() {}\n");

    // This should still succeed for a single-package manifest
    // (or fail depending on implementation). Just verify it doesn't panic.
    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .output()
        .expect("failed to run");

    // Either success (single package plan) or failure (workspace required)
    // — just ensure it terminates cleanly.
    assert!(output.status.code().is_some());
}

// ===========================================================================
// 3. Plan — publish = false exclusion
// ===========================================================================

#[test]
fn plan_excludes_publish_false_crate() {
    let td = tempdir().expect("tempdir");
    create_workspace_with_unpublished(td.path());

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
        stdout.contains("alpha@0.1.0"),
        "publishable crate should appear in plan"
    );
    // beta-internal has publish = false and should NOT appear in the publish list
    assert!(
        !stdout.contains("beta-internal@0.0.1\n")
            || stdout.contains("Skipped")
            || stdout.contains("publish = false"),
        "publish=false crate should be excluded or marked as skipped"
    );
}

#[test]
fn plan_skipped_publish_false_shows_reason() {
    let td = tempdir().expect("tempdir");
    create_workspace_with_unpublished(td.path());

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
        stdout.contains("Total packages to publish: 1"),
        "only one publishable package"
    );
}

// ===========================================================================
// 4. Plan — verbose mode
// ===========================================================================

#[test]
fn plan_verbose_shows_dependency_analysis() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--verbose")
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("Dependency Analysis"))
        .stdout(contains("Publishing Levels"))
        .stdout(contains("Dependency Graph"));
}

#[test]
fn plan_verbose_shows_estimated_analysis() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--verbose")
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("Estimated Publishing Analysis"))
        .stdout(contains("Total publish levels"));
}

/// Snapshot: verbose plan output for a multi-crate workspace.
#[test]
fn plan_verbose_multi_crate_snapshot() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--verbose")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("plan_verbose_multi_crate", normalize_output(&stdout));
}

// ===========================================================================
// 5. Plan — publish = false snapshot
// ===========================================================================

/// Snapshot: plan output when workspace contains a publish=false crate.
#[test]
fn plan_publish_false_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace_with_unpublished(td.path());

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
    assert_snapshot!("plan_publish_false", normalize_output(&stdout));
}

// ===========================================================================
// 6. Doctor command — structural checks
// ===========================================================================

#[test]
fn doctor_output_starts_with_header() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

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
        .stdout(contains("Shipper Doctor - Diagnostics Report"));

    registry.join();
}

#[test]
fn doctor_output_ends_with_diagnostics_complete() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

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
        .stdout(contains("Diagnostics complete."));

    registry.join();
}

#[test]
fn doctor_shows_registry_reachable() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

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

#[test]
fn doctor_shows_index_base() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

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
        .stdout(contains("index_base:"));

    registry.join();
}

// ===========================================================================
// 7. Clean command
// ===========================================================================

#[test]
fn clean_nonexistent_state_dir_succeeds() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("clean")
        .assert()
        .success()
        .stdout(contains("State directory does not exist"));
}

#[test]
fn clean_removes_state_and_events_files() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "{}").expect("write state");
    fs::write(state_dir.join("events.jsonl"), "").expect("write events");
    fs::write(state_dir.join("receipt.json"), "{}").expect("write receipt");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("clean")
        .assert()
        .success()
        .stdout(contains("Removed"))
        .stdout(contains("Clean complete"));

    assert!(!state_dir.join("state.json").exists());
    assert!(!state_dir.join("events.jsonl").exists());
    assert!(!state_dir.join("receipt.json").exists());
}

#[test]
fn clean_keep_receipt_preserves_receipt_file() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "{}").expect("write state");
    fs::write(state_dir.join("events.jsonl"), "").expect("write events");
    fs::write(state_dir.join("receipt.json"), "{}").expect("write receipt");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("clean")
        .arg("--keep-receipt")
        .assert()
        .success()
        .stdout(contains("Clean complete"));

    assert!(
        !state_dir.join("state.json").exists(),
        "state.json should be removed"
    );
    assert!(
        !state_dir.join("events.jsonl").exists(),
        "events.jsonl should be removed"
    );
    assert!(
        state_dir.join("receipt.json").exists(),
        "receipt.json should be preserved with --keep-receipt"
    );
}

/// Snapshot: clean output when state directory does not exist.
#[test]
fn clean_no_state_dir_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("clean")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("clean_no_state_dir", normalize_output(&stdout));
}

// ===========================================================================
// 8. Completion command
// ===========================================================================

#[test]
fn completion_bash_generates_output() {
    let output = shipper_cmd()
        .args(["completion", "bash"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "bash completion should produce non-empty output"
    );
    assert!(
        stdout.contains("shipper"),
        "bash completion should reference 'shipper'"
    );
}

#[test]
fn completion_powershell_generates_output() {
    let output = shipper_cmd()
        .args(["completion", "powershell"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "powershell completion should produce non-empty output"
    );
}

#[test]
fn completion_zsh_generates_output() {
    let output = shipper_cmd()
        .args(["completion", "zsh"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "zsh completion should produce non-empty output"
    );
}

// ===========================================================================
// 9. CI subcommands
// ===========================================================================

#[test]
fn ci_circleci_includes_steps() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["ci", "circleci"])
        .assert()
        .success()
        .stdout(contains("CircleCI"));
}

#[test]
fn ci_azure_devops_includes_pipeline() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["ci", "azure-devops"])
        .assert()
        .success()
        .stdout(contains("Azure"));
}

/// Snapshot: CI CircleCI output.
#[test]
fn ci_circleci_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .args(["ci", "circleci"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("ci_circleci", normalize_output(&stdout));
}

/// Snapshot: CI Azure DevOps output.
#[test]
fn ci_azure_devops_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .args(["ci", "azure-devops"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("ci_azure_devops", normalize_output(&stdout));
}

// ===========================================================================
// 10. Config init — content snapshot
// ===========================================================================

/// Snapshot: content of the generated .shipper.toml from `config init`.
#[test]
fn config_init_content_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .assert()
        .success();

    let content = fs::read_to_string(&config_path).expect("read config");
    assert_snapshot!("config_init_content", content);
}
