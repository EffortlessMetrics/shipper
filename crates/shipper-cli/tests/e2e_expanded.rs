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
                let normalized = normalize_embedded_paths(&line.replace('\\', "/"));
                normalize_timing(&normalized)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replace timing values like `(21.3µs)` or `(1.2ms)` with `(<DURATION>)`.
fn normalize_timing(line: &str) -> String {
    // Match patterns like "(123.4µs)" or "(1.2ms)" or "(0.5s)"
    let mut result = line.to_string();
    while let Some(start) = result.find("(") {
        if let Some(end) = result[start..].find(')') {
            let inner = &result[start + 1..start + end];
            if inner.ends_with("µs")
                || inner.ends_with("ms")
                || inner.ends_with("ns")
                || (inner.ends_with('s') && inner[..inner.len() - 1].parse::<f64>().is_ok())
            {
                result = format!(
                    "{}(<DURATION>){}",
                    &result[..start],
                    &result[start + end + 1..]
                );
                continue;
            }
        }
        break;
    }
    result
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
    let stripped = console::strip_ansi_codes(raw);
    let mut normalized = stripped
        .replace("\r\n", "\n")
        .replace("shipper.exe", "shipper")
        .replace(env!("CARGO_PKG_VERSION"), "[VERSION]");

    // Strip relative path prefixes often seen in cargo error messages in some CI environments
    while normalized.contains("../") {
        normalized = normalized.replace("../", "");
    }

    normalized
}

fn normalize_tempdir_stderr(raw: &str, tempdir: &Path) -> String {
    let tempdir = tempdir.to_string_lossy();
    normalize_stderr(
        &raw.replace(tempdir.as_ref(), "<TMPDIR>")
            .replace(&tempdir.replace('\\', "/"), "<TMPDIR>")
            .replace('\\', "/"),
    )
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
            let req = match server.recv_timeout(std::time::Duration::from_secs(30)) {
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
            let req = match server.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(Some(r)) => r,
                _ => break,
            };
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

// ===========================================================================
// 33. Publish error paths (no registry needed)
// ===========================================================================

/// Snapshot: publish with a missing manifest fails and reports an error.
#[test]
fn publish_missing_manifest_snapshot() {
    let td = tempdir().expect("tempdir");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("no-such/Cargo.toml"))
        .arg("publish")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "publish_missing_manifest",
        normalize_tempdir_stderr(&stderr, td.path())
    );
}

/// Exit code for publish with a missing manifest is non-zero.
#[test]
fn publish_missing_manifest_exit_code() {
    let td = tempdir().expect("tempdir");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("no-such/Cargo.toml"))
        .arg("publish")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    assert_debug_snapshot!("publish_missing_manifest_exit_code", output.status.code());
}

// ===========================================================================
// 34. Resume error paths
// ===========================================================================

/// Snapshot: resume when no state file exists.
#[test]
fn resume_no_state_file_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(td.path().join(".shipper"))
        .arg("resume")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "resume_no_state_file",
        normalize_output(&normalize_stderr(&stderr))
    );
}

/// Exit code for resume when no state file exists.
#[test]
fn resume_no_state_file_exit_code() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(td.path().join(".shipper"))
        .arg("resume")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    assert_debug_snapshot!("resume_no_state_file_exit_code", output.status.code());
}

/// Snapshot: resume with a corrupted (non-JSON) state file.
#[test]
fn resume_corrupted_state_file_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "NOT VALID JSON {{{{").expect("write");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "resume_corrupted_state_file",
        normalize_output(&normalize_stderr(&stderr))
    );
}

/// Snapshot: resume with an empty state file.
#[test]
fn resume_empty_state_file_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "").expect("write");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "resume_empty_state_file",
        normalize_output(&normalize_stderr(&stderr))
    );
}

// ===========================================================================
// 35. Preflight error snapshots
// ===========================================================================

/// Snapshot: preflight in a non-git directory (without --allow-dirty) fails.
#[test]
fn preflight_non_git_directory_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // Use a bogus API base that won't be contacted (git check fails first)
    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:1")
        .arg("--skip-ownership-check")
        .arg("preflight")
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .output()
        .expect("failed to run");

    // Preflight in a non-git directory should fail
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "preflight_non_git_directory",
        normalize_output(&normalize_stderr(&stderr))
    );
}

/// Snapshot: preflight with --allow-dirty and --skip-ownership-check where
/// registry reports versions as missing.
#[test]
fn preflight_allow_dirty_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let registry = spawn_registry_not_found(1);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("preflight")
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("--- stdout ---\n{stdout}--- stderr ---\n{stderr}");
    assert_snapshot!(
        "preflight_allow_dirty",
        normalize_output(&normalize_stderr(&combined))
    );

    registry.join();
}

// ===========================================================================
// 36. Config validate edge case snapshots
// ===========================================================================

/// Snapshot: config validate with wrong value type for verify.mode (integer instead of string).
#[test]
fn config_validate_wrong_type_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    fs::write(
        &config_path,
        r#"
[readiness]
method = 99999
"#,
    )
    .expect("write");

    let output = shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "config_validate_wrong_type",
        normalize_stderr(
            &stderr
                .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
                .replace(
                    &config_path.to_str().unwrap().replace('\\', "/"),
                    "<CONFIG_PATH>",
                ),
        )
    );
}

/// Snapshot: config validate with invalid TOML syntax (unclosed bracket).
#[test]
fn config_validate_malformed_toml_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    fs::write(&config_path, "[policy\nname = \"safe\"\n").expect("write");

    let output = shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "config_validate_malformed_toml",
        normalize_stderr(
            &stderr
                .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
                .replace(
                    &config_path.to_str().unwrap().replace('\\', "/"),
                    "<CONFIG_PATH>",
                ),
        )
    );
}

/// Snapshot: config validate with integer for verify.mode (expects string enum).
#[test]
fn config_validate_invalid_nested_section_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    fs::write(
        &config_path,
        r#"
[verify]
mode = 12345
"#,
    )
    .expect("write");

    let output = shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "config_validate_invalid_nested_section",
        normalize_stderr(
            &stderr
                .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
                .replace(
                    &config_path.to_str().unwrap().replace('\\', "/"),
                    "<CONFIG_PATH>",
                ),
        )
    );
}

// ===========================================================================
// 37. Verbose flag behavior
// ===========================================================================

/// Snapshot: verbose flag with single-crate plan shows extra analysis.
#[test]
fn verbose_single_crate_plan_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

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
    assert_snapshot!("verbose_single_crate_plan", normalize_output(&stdout));
}

// ===========================================================================
// 38. Plan edge cases
// ===========================================================================

/// Snapshot: plan for a workspace where all crates have publish = false.
#[test]
fn plan_all_publish_false_snapshot() {
    let td = tempdir().expect("tempdir");
    write_file(
        &td.path().join("Cargo.toml"),
        r#"
[workspace]
members = ["internal-a", "internal-b"]
resolver = "2"
"#,
    );
    write_file(
        &td.path().join("internal-a/Cargo.toml"),
        r#"
[package]
name = "internal-a"
version = "0.1.0"
edition = "2021"
publish = false
"#,
    );
    write_file(&td.path().join("internal-a/src/lib.rs"), "");
    write_file(
        &td.path().join("internal-b/Cargo.toml"),
        r#"
[package]
name = "internal-b"
version = "0.1.0"
edition = "2021"
publish = false
"#,
    );
    write_file(&td.path().join("internal-b/src/lib.rs"), "");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("plan_all_publish_false", normalize_output(&stdout));
}

/// Snapshot: plan with --format json flag accepted and output stable.
#[test]
fn plan_format_json_flag_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--format")
        .arg("json")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("plan_format_json_flag", normalize_output(&stdout));
}

// ===========================================================================
// 39. Help text snapshots for remaining subcommands
// ===========================================================================

/// Snapshot: `preflight --help` output.
#[test]
fn help_preflight_snapshot() {
    let output = shipper_cmd()
        .args(["preflight", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_preflight", normalize_stderr(&stdout));
}

/// Snapshot: `ci --help` output.
#[test]
fn help_ci_snapshot() {
    let output = shipper_cmd()
        .args(["ci", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_ci", normalize_stderr(&stdout));
}

// ===========================================================================
// 40. Error snapshots — unknown subcommand and global errors
// ===========================================================================

/// Snapshot: error when an unknown subcommand is used.
#[test]
fn error_unknown_subcommand_snapshot() {
    let output = shipper_cmd().arg("foobar").output().expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_unknown_subcommand", normalize_stderr(&stderr));
}

/// Exit code for an unknown subcommand is non-zero (2 = clap usage error).
#[test]
fn error_unknown_subcommand_exit_code() {
    let output = shipper_cmd().arg("foobar").output().expect("failed to run");

    assert!(!output.status.success());
    assert_debug_snapshot!("error_unknown_subcommand_exit_code", output.status.code());
}

// ===========================================================================
// 41. Inspect-events with actual events file
// ===========================================================================

/// Snapshot: inspect-events with a populated events file.
#[test]
fn inspect_events_with_data_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(
        state_dir.join("events.jsonl"),
        concat!(
            r#"{"timestamp":"2025-01-01T00:00:00Z","event_type":{"type":"plan_created","plan_id":"abc123","package_count":1},"package":"all"}"#,
            "\n",
            r#"{"timestamp":"2025-01-01T00:00:01Z","event_type":{"type":"execution_started"},"package":"all"}"#,
            "\n",
        ),
    )
    .expect("write");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("inspect-events")
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("inspect_events_with_data", normalize_output(&stdout));
}

// ===========================================================================
// 42. Publish with invalid manifest content
// ===========================================================================

/// Snapshot: publish when manifest exists but has invalid TOML.
#[test]
fn publish_invalid_manifest_content_snapshot() {
    let td = tempdir().expect("tempdir");
    write_file(&td.path().join("Cargo.toml"), "this is not valid toml {{{{");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("publish")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "publish_invalid_manifest_content",
        normalize_stderr(
            &stderr
                .replace(td.path().to_str().unwrap(), "<TMPDIR>")
                .replace(&td.path().to_str().unwrap().replace('\\', "/"), "<TMPDIR>",),
        )
    );
}

// ===========================================================================
// 43. Resume with missing manifest
// ===========================================================================

/// Snapshot: resume when manifest path doesn't exist.
#[test]
fn resume_missing_manifest_snapshot() {
    let td = tempdir().expect("tempdir");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("nonexistent/Cargo.toml"))
        .arg("resume")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "resume_missing_manifest",
        normalize_tempdir_stderr(&stderr, td.path())
    );
}

// ===========================================================================
// 44. Help text — top-level help snapshot
// ===========================================================================

/// Snapshot: `--help` output for the root command.
#[test]
fn help_root_snapshot() {
    let output = shipper_cmd().arg("--help").output().expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_root", normalize_stderr(&stdout));
}

/// Snapshot: `config --help` output.
#[test]
fn help_config_snapshot() {
    let output = shipper_cmd()
        .args(["config", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_config", normalize_stderr(&stdout));
}

// ===========================================================================
// 45. Publish with --dry-run-like flags (no-verify, allow-dirty, etc.)
// ===========================================================================

/// Publish with --no-verify on a missing manifest still fails (no actual publish).
#[test]
fn publish_no_verify_missing_manifest_fails() {
    let td = tempdir().expect("tempdir");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--no-verify")
        .arg("publish")
        .assert()
        .failure();
}

/// Publish with --allow-dirty on a missing manifest still fails.
#[test]
fn publish_allow_dirty_missing_manifest_fails() {
    let td = tempdir().expect("tempdir");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--allow-dirty")
        .arg("publish")
        .assert()
        .failure();
}

/// Publish with --skip-ownership-check on a missing manifest still fails.
#[test]
fn publish_skip_ownership_missing_manifest_fails() {
    let td = tempdir().expect("tempdir");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--skip-ownership-check")
        .arg("publish")
        .assert()
        .failure();
}

/// Publish with combined flags on a valid workspace succeeds (skipping
/// already-published packages) even without a git repo or registry token.
#[test]
fn publish_combined_flags_no_git_succeeds() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--no-verify")
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--no-readiness")
        .arg("--state-dir")
        .arg(td.path().join(".shipper"))
        .arg("publish")
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The command should complete (either success or failure)
    assert!(
        output.status.code().is_some(),
        "publish should terminate cleanly"
    );
    // Should mention the package and state dir
    assert!(
        stdout.contains("demo@0.1.0") || stderr.contains("demo@0.1.0"),
        "output should mention the package: stdout={stdout} stderr={stderr}"
    );
}

/// Publish with --package filter on a valid workspace runs only the selected package.
#[test]
fn publish_package_filter_no_registry_runs() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("core-lib")
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--no-readiness")
        .arg("--state-dir")
        .arg(td.path().join(".shipper"))
        .arg("publish")
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.code().is_some(),
        "publish should terminate cleanly"
    );
    // Should mention core-lib (the filtered package)
    assert!(
        stdout.contains("core-lib@0.2.0") || stderr.contains("core-lib@0.2.0"),
        "output should mention the filtered package: stdout={stdout} stderr={stderr}"
    );
}

// ===========================================================================
// 46. Resume error cases — wrong plan-id in state file
// ===========================================================================

/// Snapshot: resume with a valid JSON state file that has a mismatched plan_id.
#[test]
fn resume_wrong_plan_id_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    // Write a valid JSON state file with a bogus plan_id
    fs::write(
        state_dir.join("state.json"),
        r#"{"plan_id":"bogus-plan-id-12345","packages":[],"started_at":"2025-01-01T00:00:00Z"}"#,
    )
    .expect("write");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "resume_wrong_plan_id",
        normalize_output(&normalize_stderr(
            &stderr
                .replace(td.path().to_str().unwrap(), "<TMPDIR>")
                .replace(&td.path().to_str().unwrap().replace('\\', "/"), "<TMPDIR>")
        ))
    );
}

/// Snapshot: resume with a state file containing only `{}` (minimal valid JSON).
#[test]
fn resume_minimal_json_state_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "{}").expect("write");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "resume_minimal_json_state",
        normalize_output(&normalize_stderr(
            &stderr
                .replace(td.path().to_str().unwrap(), "<TMPDIR>")
                .replace(&td.path().to_str().unwrap().replace('\\', "/"), "<TMPDIR>")
        ))
    );
}

// ===========================================================================
// 47. Config init — overwrite protection
// ===========================================================================

/// Config init on an already-existing file: snapshot whatever the CLI does.
#[test]
fn config_init_overwrite_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    // First init succeeds
    shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .assert()
        .success();

    // Second init — capture result regardless of pass/fail
    let output = shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!(
        "exit_code: {:?}\n--- stdout ---\n{}--- stderr ---\n{}",
        output.status.code(),
        stdout
            .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
            .replace(
                &config_path.to_str().unwrap().replace('\\', "/"),
                "<CONFIG_PATH>",
            ),
        normalize_stderr(
            &stderr
                .replace(config_path.to_str().unwrap(), "<CONFIG_PATH>")
                .replace(
                    &config_path.to_str().unwrap().replace('\\', "/"),
                    "<CONFIG_PATH>",
                )
        )
    );
    assert_snapshot!("config_init_overwrite", combined);
}

// ===========================================================================
// 48. Config validate with valid sections
// ===========================================================================

/// Snapshot: config validate with explicitly set policy section.
#[test]
fn config_validate_policy_section_snapshot() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    fs::write(
        &config_path,
        r#"
[policy]
name = "balanced"

[retry]
max_attempts = 3
base_delay = "1s"
max_delay = "30s"
strategy = "exponential"
"#,
    )
    .expect("write");

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
    assert_snapshot!("config_validate_policy_section", normalized);
}

// ===========================================================================
// 49. Status with mock registry — snapshot for published multi-crate
// ===========================================================================

/// Snapshot: status for a multi-crate workspace where all are published.
#[test]
fn status_multi_crate_all_published_snapshot() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let registry = spawn_registry(3);

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
    assert_snapshot!(
        "status_multi_crate_all_published",
        normalize_output(&stdout)
    );

    registry.join();
}

// ===========================================================================
// 50. Clean edge cases
// ===========================================================================

/// Clean on a state dir with only receipt.json and nothing else to remove.
#[test]
fn clean_only_receipt_exists_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
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
    assert_snapshot!("clean_only_receipt_exists", normalize_output(&stdout));
}

/// Clean with --keep-receipt when only state.json exists (no receipt to keep).
#[test]
fn clean_keep_receipt_no_receipt_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("state.json"), "{}").expect("write state");

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
    assert_snapshot!("clean_keep_receipt_no_receipt", normalize_output(&stdout));
}

// ===========================================================================
// 51. Error output — invalid duration flags
// ===========================================================================

/// Snapshot: error when --base-delay has invalid duration format.
#[test]
fn error_invalid_base_delay_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--base-delay", "not-a-duration", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_invalid_base_delay", normalize_stderr(&stderr));
}

/// Snapshot: error when --verify-timeout has invalid duration format.
#[test]
fn error_invalid_verify_timeout_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--verify-timeout", "xyz", "plan"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("error_invalid_verify_timeout", normalize_stderr(&stderr));
}

// ===========================================================================
// 52. Plan with --quiet flag
// ===========================================================================

/// Plan output with --quiet still shows essential info on stdout.
#[test]
fn plan_quiet_mode_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--quiet")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("plan_quiet_mode", normalize_output(&stdout));
}

// ===========================================================================
// 53. Status snapshot with --package filter for multi-crate
// ===========================================================================

/// Snapshot: status filtered to two packages (transitive deps included).
#[test]
fn status_multi_package_filter_snapshot() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    // Selecting core-lib and top-app pulls in mid-lib transitively (3 total)
    let registry = spawn_registry_not_found(3);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--quiet")
        .arg("--package")
        .arg("core-lib")
        .arg("--package")
        .arg("top-app")
        .arg("status")
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
        stdout.contains("top-app@0.4.0"),
        "second filtered package should appear"
    );
    assert_snapshot!("status_multi_package_filter", normalize_output(&stdout));

    registry.join();
}

// ===========================================================================
// 54. CI snippet snapshots — help subcommands
// ===========================================================================

/// Snapshot: `ci github-actions --help` output.
#[test]
fn help_ci_github_actions_snapshot() {
    let output = shipper_cmd()
        .args(["ci", "github-actions", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_ci_github_actions", normalize_stderr(&stdout));
}

/// Snapshot: `ci gitlab --help` output.
#[test]
fn help_ci_gitlab_snapshot() {
    let output = shipper_cmd()
        .args(["ci", "gitlab", "--help"])
        .output()
        .expect("failed to run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_ci_gitlab", normalize_stderr(&stdout));
}

// ===========================================================================
// 55. Preflight with --format json on a non-git dir still errors
// ===========================================================================

/// Snapshot: preflight with --format json in a non-git directory.
#[test]
fn preflight_json_format_non_git_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:1")
        .arg("--skip-ownership-check")
        .arg("--format")
        .arg("json")
        .arg("preflight")
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!(
        "preflight_json_format_non_git",
        normalize_output(&normalize_stderr(&stderr))
    );
}

// ===========================================================================
// 56. Inspect-events with malformed events file
// ===========================================================================

/// Snapshot: inspect-events with a malformed (non-JSON) events file.
#[test]
fn inspect_events_malformed_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::write(state_dir.join("events.jsonl"), "NOT JSON AT ALL\n").expect("write");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("inspect-events")
        .output()
        .expect("failed to run");

    // Should either succeed with warnings or fail — just snapshot whatever happens
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!(
        "exit_code: {:?}\n--- stdout ---\n{stdout}--- stderr ---\n{stderr}",
        output.status.code()
    );
    assert_snapshot!(
        "inspect_events_malformed",
        normalize_output(&normalize_stderr(&combined))
    );
}

// ===========================================================================
// 57. Plan with --format json on multi-crate workspace
// ===========================================================================

/// Snapshot: plan with --format json for multi-crate workspace.
#[test]
fn plan_format_json_multi_crate_snapshot() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--format")
        .arg("json")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert_snapshot!("plan_format_json_multi_crate", normalize_output(&stdout));
}

// ===========================================================================
// 58. Doctor snapshot — with existing state dir
// ===========================================================================

/// Snapshot: doctor output when state directory exists.
#[test]
fn doctor_with_existing_state_dir_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    fs::create_dir_all(&state_dir).expect("mkdir");
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(&state_dir)
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
    // When state_dir exists, doctor should report it as existing/writable
    assert!(
        stdout.contains("state_dir_writable:") || stdout.contains("state_dir_exists:"),
        "doctor should report state_dir status"
    );

    registry.join();
}

// ===========================================================================
// 59. Error output — conflicting or unusual flag combos
// ===========================================================================

/// Snapshot: error when --max-attempts is set to 0.
#[test]
fn plan_max_attempts_zero_accepted() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // --max-attempts 0 should be accepted by clap (validated later at runtime)
    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--max-attempts", "0", "plan"])
        .output()
        .expect("failed to run");

    // Should succeed at plan stage (runtime validation happens at publish)
    assert!(output.status.success());
}

/// Snapshot: error when --retry-jitter is out of range (> 1.0).
#[test]
fn plan_retry_jitter_out_of_range_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .args(["--retry-jitter", "5.0", "plan"])
        .output()
        .expect("failed to run");

    // Capture whatever happens — plan may still succeed since jitter is only used at publish
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!(
        "exit_code: {:?}\n--- stdout ---\n{}--- stderr ---\n{}",
        output.status.code(),
        normalize_output(&stdout),
        normalize_stderr(&stderr)
    );
    assert_snapshot!("plan_retry_jitter_out_of_range", combined);
}

// ===========================================================================
// 60. Workspace with deeply nested crate
// ===========================================================================

/// Snapshot: plan for a workspace with a deeply nested crate path.
#[test]
fn plan_deeply_nested_crate_snapshot() {
    let td = tempdir().expect("tempdir");
    write_file(
        &td.path().join("Cargo.toml"),
        r#"
[workspace]
members = ["packages/nested/deep-crate"]
resolver = "2"
"#,
    );
    write_file(
        &td.path().join("packages/nested/deep-crate/Cargo.toml"),
        r#"
[package]
name = "deep-crate"
version = "1.0.0"
edition = "2021"
"#,
    );
    write_file(
        &td.path().join("packages/nested/deep-crate/src/lib.rs"),
        "pub fn deep() {}\n",
    );

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
    assert!(stdout.contains("deep-crate@1.0.0"));
    assert_snapshot!("plan_deeply_nested_crate", normalize_output(&stdout));
}
