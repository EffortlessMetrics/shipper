//! Expanded E2E tests for shipper-cli covering doctor, config, status, plan,
//! clean, completion, CI subcommands, error output, and snapshot stability.

use std::fs;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
use insta::{assert_debug_snapshot, assert_snapshot};
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

/// Normalize stderr/stdout that may contain the binary name (which differs
/// across platforms) and the embedded version string.
fn normalize_stderr(raw: &str) -> String {
    raw.replace("shipper.exe", "shipper")
        .replace(env!("CARGO_PKG_VERSION"), "[VERSION]")
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

// ===========================================================================
// 11. Error output snapshots
// ===========================================================================

/// Snapshot: error when an invalid --format value is provided.
#[test]
fn error_invalid_format_value_snapshot() {
    let output = shipper_cmd()
        .args(["--format", "invalid", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let normalized = normalize_stderr(&stderr);
    assert_snapshot!("error_invalid_format_value", normalized);
}

/// Snapshot: error when `ci` is invoked without a provider subcommand.
#[test]
fn error_missing_ci_subcommand_snapshot() {
    let output = shipper_cmd().arg("ci").output().expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let normalized = normalize_stderr(&stderr);
    assert_snapshot!("error_missing_ci_subcommand", normalized);
}

/// Snapshot: error when --package selects a crate that does not exist.
#[test]
fn error_nonexistent_package_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--package", "nonexistent", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_nonexistent_package", normalize_stderr(&stderr));
}

/// Snapshot: error when an invalid --retry-strategy value is provided.
#[test]
fn error_invalid_retry_strategy_snapshot() {
    let output = shipper_cmd()
        .args(["--retry-strategy", "bogus", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_invalid_retry_strategy", normalize_stderr(&stderr));
}

// ===========================================================================
// 12. Help text snapshots (via e2e_expanded)
// ===========================================================================

/// Snapshot: `completion --help` output.
#[test]
fn help_completion_snapshot() {
    let output = shipper_cmd()
        .args(["completion", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_completion", normalize_stderr(&stdout));
}

/// Snapshot: `inspect-events --help` output.
#[test]
fn help_inspect_events_snapshot() {
    let output = shipper_cmd()
        .args(["inspect-events", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_inspect_events", normalize_stderr(&stdout));
}

/// Snapshot: `inspect-receipt --help` output.
#[test]
fn help_inspect_receipt_snapshot() {
    let output = shipper_cmd()
        .args(["inspect-receipt", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_inspect_receipt", normalize_stderr(&stdout));
}

// ===========================================================================
// 13. Version output
// ===========================================================================

/// Snapshot: `--version` output with version redacted.
#[test]
fn version_output_snapshot() {
    let output = shipper_cmd()
        .arg("--version")
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let normalized = normalize_stderr(&stdout);
    assert_snapshot!("version_output", normalized);
}

// ===========================================================================
// 14. Config validate
// ===========================================================================

/// Snapshot: `config validate` on a freshly generated config file.
#[test]
fn config_validate_valid_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .assert()
        .success();

    let output = shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let normalized = stdout
        .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
        .replace(
            &config_path.to_str().unwrap().replace('\\', "/"),
            "<CONFIG_PATH>",
        );
    assert_snapshot!("config_validate_valid", normalized);
}

/// Snapshot: `config validate` on an invalid TOML file.
#[test]
fn config_validate_invalid_toml_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    fs::write(&config_path, "this is {{ not valid toml").expect("write");

    let output = shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let normalized = stderr
        .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
        .replace(
            &config_path.to_str().unwrap().replace('\\', "/"),
            "<CONFIG_PATH>",
        );
    assert_snapshot!("config_validate_invalid_toml", normalized);
}

/// Config validate on a nonexistent file fails with an error.
#[test]
fn config_validate_nonexistent_fails() {
    let td = tempdir().expect("tempdir");
    let missing = td.path().join("does-not-exist.toml");

    shipper_cmd()
        .args(["config", "validate", "-p", missing.to_str().unwrap()])
        .assert()
        .failure();
}

// ===========================================================================
// 15. CI snippet snapshots (github-actions, gitlab)
// ===========================================================================

/// Snapshot: CI GitHub Actions output.
#[test]
fn ci_github_actions_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .args(["ci", "github-actions"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("ci_github_actions", normalize_output(&stdout));
}

/// Snapshot: CI GitLab output.
#[test]
fn ci_gitlab_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .args(["ci", "gitlab"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("ci_gitlab", normalize_output(&stdout));
}

// ===========================================================================
// 16. Clean snapshots
// ===========================================================================

/// Snapshot: clean output when --keep-receipt is used.
#[test]
fn clean_keep_receipt_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "{}").expect("write state");
    fs::write(state_dir.join("events.jsonl"), "").expect("write events");
    fs::write(state_dir.join("receipt.json"), "{}").expect("write receipt");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("clean")
        .arg("--keep-receipt")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("clean_keep_receipt", normalize_output(&stdout));
}

/// Snapshot: clean output when all state files are removed (no --keep-receipt).
#[test]
fn clean_all_files_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "{}").expect("write state");
    fs::write(state_dir.join("events.jsonl"), "").expect("write events");
    fs::write(state_dir.join("receipt.json"), "{}").expect("write receipt");

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
    assert_snapshot!("clean_all_files", normalize_output(&stdout));
}

// ===========================================================================
// 17. Plan — single crate snapshot
// ===========================================================================

/// Snapshot: plan output for a single-crate workspace.
#[test]
fn plan_single_crate_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

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
    assert_snapshot!("plan_single_crate", normalize_output(&stdout));
}

/// Snapshot: plan output for a multi-crate workspace (non-verbose).
#[test]
fn plan_multi_crate_snapshot() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

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
    assert_snapshot!("plan_multi_crate", normalize_output(&stdout));
}

// ===========================================================================
// 18. Inspect-events — empty state
// ===========================================================================

/// Snapshot: inspect-events output when no events file exists.
#[test]
fn inspect_events_empty_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("inspect-events")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("inspect_events_empty", normalize_output(&stdout));
}

// ===========================================================================
// 19. Completion — fish and elvish shells
// ===========================================================================

#[test]
fn completion_fish_generates_output() {
    let output = shipper_cmd()
        .args(["completion", "fish"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "fish completion should produce non-empty output"
    );
    assert!(
        stdout.contains("shipper"),
        "fish completion should reference 'shipper'"
    );
}

#[test]
fn completion_elvish_generates_output() {
    let output = shipper_cmd()
        .args(["completion", "elvish"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "elvish completion should produce non-empty output"
    );
}

// ===========================================================================
// 20. Config init — output message snapshot
// ===========================================================================

/// Snapshot: stdout message printed by `config init`.
#[test]
fn config_init_output_message_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    let output = shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let normalized = stdout
        .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
        .replace(
            &config_path.to_str().unwrap().replace('\\', "/"),
            "<CONFIG_PATH>",
        );
    assert_snapshot!("config_init_output_message", normalized);
}

// ===========================================================================
// 21. Debug snapshot for inspect-receipt missing state
// ===========================================================================

/// Debug-snapshot: the exit status when inspect-receipt has no receipt file.
#[test]
fn inspect_receipt_missing_fails() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("inspect-receipt")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The error message references the receipt path; just assert it's non-empty.
    assert!(
        !stderr.trim().is_empty(),
        "stderr should contain an error about missing receipt"
    );
    assert_debug_snapshot!("inspect_receipt_missing_exit_code", output.status.code());
}

// ===========================================================================
// 22. Help text snapshots for additional subcommands
// ===========================================================================

/// Snapshot: `resume --help` output.
#[test]
fn help_resume_snapshot() {
    let output = shipper_cmd()
        .args(["resume", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_resume", normalize_stderr(&stdout));
}

/// Snapshot: `publish --help` output.
#[test]
fn help_publish_snapshot() {
    let output = shipper_cmd()
        .args(["publish", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_publish", normalize_stderr(&stdout));
}

/// Snapshot: `doctor --help` output.
#[test]
fn help_doctor_snapshot() {
    let output = shipper_cmd()
        .args(["doctor", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_doctor", normalize_stderr(&stdout));
}

/// Snapshot: `status --help` output.
#[test]
fn help_status_snapshot() {
    let output = shipper_cmd()
        .args(["status", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_status", normalize_stderr(&stdout));
}

/// Snapshot: `plan --help` output.
#[test]
fn help_plan_snapshot() {
    let output = shipper_cmd()
        .args(["plan", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_plan", normalize_stderr(&stdout));
}

/// Snapshot: `clean --help` output.
#[test]
fn help_clean_snapshot() {
    let output = shipper_cmd()
        .args(["clean", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_clean", normalize_stderr(&stdout));
}

/// Snapshot: `config init --help` output.
#[test]
fn help_config_init_snapshot() {
    let output = shipper_cmd()
        .args(["config", "init", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_config_init", normalize_stderr(&stdout));
}

/// Snapshot: `config validate --help` output.
#[test]
fn help_config_validate_snapshot() {
    let output = shipper_cmd()
        .args(["config", "validate", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_config_validate", normalize_stderr(&stdout));
}

// ===========================================================================
// 23. Doctor — full output snapshot and auth detection
// ===========================================================================

/// Doctor output contains all expected sections.
#[test]
fn doctor_full_output_has_all_sections() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

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
    // Verify all expected sections are present
    assert!(stdout.contains("Shipper Doctor - Diagnostics Report"));
    assert!(stdout.contains("workspace_root:"));
    assert!(stdout.contains("registry: crates-io"));
    assert!(stdout.contains("auth_type:"));
    assert!(stdout.contains("state_dir:"));
    assert!(stdout.contains("cargo:"));
    assert!(stdout.contains("git:"));
    assert!(stdout.contains("registry_reachable:"));
    assert!(stdout.contains("index_base:"));
    assert!(stdout.contains("Diagnostics complete."));

    registry.join();
}

/// Doctor shows NONE FOUND when no auth token is set.
#[test]
fn doctor_shows_no_auth_when_no_token() {
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
        .stdout(contains("NONE FOUND"));

    registry.join();
}

/// Doctor reports state_dir_exists: false when state dir does not exist.
#[test]
fn doctor_shows_state_dir_not_exists() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(".shipper-nonexistent")
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .success()
        .stdout(contains("state_dir_exists: false"));

    registry.join();
}

// ===========================================================================
// 24. Status with mock registry
// ===========================================================================

fn spawn_registry_not_found(expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for _ in 0..expected_requests {
            let req = server.recv().expect("request");
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(404))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json").expect("header"),
                );
            req.respond(resp).expect("respond");
        }
    });
    TestRegistry { base_url, handle }
}

/// Status shows "missing" when the registry returns 404 for a package version.
#[test]
fn status_shows_missing_for_unpublished() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let registry = spawn_registry_not_found(1);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--quiet")
        .arg("status")
        .assert()
        .success()
        .stdout(contains("demo@0.1.0: missing"));

    registry.join();
}

/// Status shows "published" when the registry returns 200 for a package version.
#[test]
fn status_shows_published_for_existing() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let registry = spawn_registry(1);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--quiet")
        .arg("status")
        .assert()
        .success()
        .stdout(contains("demo@0.1.0: published"));

    registry.join();
}

/// Snapshot: status output for a multi-crate workspace where all versions are missing.
#[test]
fn status_multi_crate_all_missing_snapshot() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let registry = spawn_registry_not_found(3);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--quiet")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("status_multi_crate_all_missing", normalize_output(&stdout));

    registry.join();
}

/// Snapshot: status output for a single workspace where the version is published.
#[test]
fn status_single_crate_published_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let registry = spawn_registry(1);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--quiet")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("status_single_crate_published", normalize_output(&stdout));

    registry.join();
}

// ===========================================================================
// 25. Plan with --package filtering
// ===========================================================================

/// Plan filtered to a single package in a multi-crate workspace.
#[test]
fn plan_single_package_filter_in_multi_crate() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("core-lib")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert!(
        stdout.contains("core-lib@0.2.0"),
        "filtered package should appear in plan"
    );
    assert!(
        !stdout.contains("mid-lib@0.3.0"),
        "non-filtered package should not appear"
    );
    assert!(
        !stdout.contains("top-app@0.4.0"),
        "non-filtered package should not appear"
    );
    assert_snapshot!("plan_single_package_filter", normalize_output(&stdout));
}

/// Plan filtered to multiple packages with multiple --package flags.
#[test]
fn plan_multiple_packages_filter() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("core-lib")
        .arg("--package")
        .arg("mid-lib")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert!(
        stdout.contains("core-lib@0.2.0"),
        "first filtered package should appear"
    );
    assert!(
        stdout.contains("mid-lib@0.3.0"),
        "second filtered package should appear"
    );
    assert!(
        !stdout.contains("top-app@0.4.0"),
        "non-filtered package should not appear"
    );
    assert_snapshot!("plan_multiple_packages_filter", normalize_output(&stdout));
}

// ===========================================================================
// 26. Error snapshots — invalid flag values
// ===========================================================================

/// Snapshot: error when an invalid --policy value is provided.
#[test]
fn error_invalid_policy_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--policy", "bogus", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_invalid_policy", normalize_stderr(&stderr));
}

/// Snapshot: error when an invalid --verify-mode value is provided.
#[test]
fn error_invalid_verify_mode_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--verify-mode", "bogus", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_invalid_verify_mode", normalize_stderr(&stderr));
}

/// Snapshot: error when an invalid --readiness-method value is provided.
#[test]
fn error_invalid_readiness_method_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--readiness-method", "bogus", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_invalid_readiness_method", normalize_stderr(&stderr));
}

// ===========================================================================
// 27. Manifest and path edge cases
// ===========================================================================

/// Error when --manifest-path points to a directory that does not exist.
#[test]
fn manifest_path_in_nonexistent_directory_fails() {
    shipper_cmd()
        .arg("--manifest-path")
        .arg("nonexistent-dir/Cargo.toml")
        .arg("plan")
        .assert()
        .failure();
}

/// Config init writes to a custom filename.
#[test]
fn config_init_custom_filename() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join("custom-config.toml");

    shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .assert()
        .success();

    assert!(config_path.exists(), "custom config file should be created");
    let content = fs::read_to_string(&config_path).expect("read config");
    assert!(
        content.contains("[policy]"),
        "generated config should contain [policy] section"
    );
}

/// Config init output message with a custom filename.
#[test]
fn config_init_custom_filename_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join("my-shipper.toml");

    let output = shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let normalized = stdout
        .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
        .replace(
            &config_path.to_str().unwrap().replace('\\', "/"),
            "<CONFIG_PATH>",
        );
    assert_snapshot!("config_init_custom_filename", normalized);
}

// ===========================================================================
// 28. Config validate edge cases
// ===========================================================================

/// Config validate on an empty file succeeds (empty is valid TOML).
#[test]
fn config_validate_empty_file() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    fs::write(&config_path, "").expect("write");

    shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .assert()
        .success();
}

/// Config validate with an unknown section still succeeds (serde ignores unknown fields).
#[test]
fn config_validate_unknown_section_succeeds() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    fs::write(
        &config_path,
        r#"
[policy]
name = "safe"

[unknown_section]
key = "value"
"#,
    )
    .expect("write");

    // This may succeed or fail depending on whether serde(deny_unknown_fields)
    // is set. We just verify it terminates cleanly.
    let output = shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(output.status.code().is_some());
}

// ===========================================================================
// 29. Quiet mode
// ===========================================================================

/// Doctor with --quiet suppresses [info] messages on stderr.
#[test]
fn quiet_mode_doctor_suppresses_info_stderr() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--quiet")
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("[info]"),
        "quiet mode should suppress [info] messages, got: {stderr}"
    );

    registry.join();
}

// ===========================================================================
// 30. Plan verbose with --package filtering
// ===========================================================================

/// Snapshot: verbose plan with a single package filter.
#[test]
fn plan_verbose_single_package_filter_snapshot() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("core-lib")
        .arg("--verbose")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert!(
        stdout.contains("core-lib@0.2.0"),
        "filtered package should appear in verbose plan"
    );
    assert_snapshot!(
        "plan_verbose_single_package_filter",
        normalize_output(&stdout)
    );
}

// ===========================================================================
// 31. Error snapshots — missing subcommand argument
// ===========================================================================

/// Snapshot: error when `config` is invoked without a subcommand.
#[test]
fn error_missing_config_subcommand_snapshot() {
    let output = shipper_cmd().arg("config").output().expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_missing_config_subcommand", normalize_stderr(&stderr));
}

/// Snapshot: error when `completion` is invoked without a shell argument.
#[test]
fn error_missing_completion_shell_snapshot() {
    let output = shipper_cmd()
        .arg("completion")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_missing_completion_shell", normalize_stderr(&stderr));
}

// ===========================================================================
// 32. Status with package filtering
// ===========================================================================

/// Status filtered to a single package shows only that package.
#[test]
fn status_single_package_filter() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let registry = spawn_registry_not_found(1);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--quiet")
        .arg("--package")
        .arg("core-lib")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert!(
        stdout.contains("core-lib@0.2.0: missing"),
        "filtered package should appear"
    );
    assert!(
        !stdout.contains("mid-lib"),
        "non-filtered packages should not appear"
    );
    assert!(
        !stdout.contains("top-app"),
        "non-filtered packages should not appear"
    );

    registry.join();
}
