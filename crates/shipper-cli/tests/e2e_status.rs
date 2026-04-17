use std::fs;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
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

/// Spawn a mock registry that responds with the given HTTP status codes.
/// `statuses` is cycled for each request; `expected_requests` is how many
/// requests the mock will serve before shutting down.
fn spawn_registry(statuses: Vec<u16>, expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for idx in 0..expected_requests {
            let req = match server.recv_timeout(std::time::Duration::from_secs(30)) {
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

/// Create a simple workspace with a single crate.
fn create_simple_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["alpha"]
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

// ── status on a simple workspace ─────────────────────────────────────

#[test]
fn status_simple_workspace_shows_local_versions() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    // Registry returns 404 → version not found → "missing"
    let registry = spawn_registry(vec![404], 1);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0: missing"));

    registry.join();
}

#[test]
fn status_workspace_shows_published_when_registry_has_version() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    // Registry returns 200 → version exists → "published"
    let registry = spawn_registry(vec![200], 1);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0: published"));

    registry.join();
}

// ── status on a multi-crate workspace ────────────────────────────────

#[test]
fn status_multi_crate_workspace() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    // 3 crates, all missing
    let registry = spawn_registry(vec![404], 3);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("core-lib@0.2.0: missing"))
        .stdout(contains("mid-lib@0.3.0: missing"))
        .stdout(contains("top-app@0.4.0: missing"));

    registry.join();
}

// ── non-workspace directory ──────────────────────────────────────────

#[test]
fn status_non_workspace_directory_fails() {
    let td = tempdir().expect("tempdir");
    write_file(&td.path().join("README.md"), "not a workspace");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("status")
        .assert()
        .failure();
}

// ── --manifest-path with explicit path ───────────────────────────────

#[test]
fn status_explicit_manifest_path() {
    let td = tempdir().expect("tempdir");
    let nested = td.path().join("nested").join("project");
    create_simple_workspace(&nested);
    let registry = spawn_registry(vec![404], 1);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(nested.join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0"));

    registry.join();
}

// ── --package filter ─────────────────────────────────────────────────

#[test]
fn status_package_filter_single() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    // Only one crate should be queried
    let registry = spawn_registry(vec![404], 1);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--package")
        .arg("core-lib")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert!(stdout.contains("core-lib@0.2.0"));
    // Filtered-out packages must not appear
    assert!(!stdout.contains("mid-lib"));
    assert!(!stdout.contains("top-app"));

    registry.join();
}

#[test]
fn status_package_filter_multiple() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    // Two crates queried
    let registry = spawn_registry(vec![404], 2);

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--package")
        .arg("core-lib")
        .arg("--package")
        .arg("mid-lib")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert!(stdout.contains("core-lib@0.2.0"));
    assert!(stdout.contains("mid-lib@0.3.0"));
    assert!(!stdout.contains("top-app"));

    registry.join();
}

// ── output format verification ───────────────────────────────────────

#[test]
fn status_output_contains_plan_id() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    let registry = spawn_registry(vec![404], 1);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("status")
        .assert()
        .success()
        .stdout(contains("plan_id: "));

    registry.join();
}

#[test]
fn status_output_format_name_at_version_colon_status() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    let registry = spawn_registry(vec![404], 1);

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

    // Verify each package line matches the "name@version: status" pattern
    let pkg_lines: Vec<&str> = stdout.lines().filter(|l| l.contains('@')).collect();

    assert!(
        !pkg_lines.is_empty(),
        "should have at least one package line"
    );
    for line in &pkg_lines {
        assert!(
            line.contains(": published") || line.contains(": missing"),
            "package line should end with ': published' or ': missing', got: {line}"
        );
    }

    registry.join();
}

#[test]
fn status_mixed_published_and_missing() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    // First crate published (200), remaining two missing (404)
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
    // At least one published and one missing
    assert!(
        stdout.contains("published"),
        "expected at least one published crate"
    );
    assert!(
        stdout.contains("missing"),
        "expected at least one missing crate"
    );

    registry.join();
}
