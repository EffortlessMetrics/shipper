//! End-to-end tests for the `shipper doctor` command.

use std::fs;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. Doctor shows cargo version
#[test]
fn doctor_shows_cargo_version() {
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
        .stdout(contains("cargo: cargo"));

    registry.join();
}

/// 2. Doctor shows rust version (via cargo version output which includes toolchain info)
#[test]
fn doctor_shows_rust_version() {
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
    // cargo version line embeds the toolchain version (e.g. "cargo 1.92.0 ...")
    let cargo_line = stdout
        .lines()
        .find(|l| l.starts_with("cargo: "))
        .expect("expected a cargo: line");
    assert!(
        cargo_line.contains('.'),
        "cargo version should contain a dot-separated version number, got: {cargo_line}"
    );

    registry.join();
}

/// 3. Doctor detects CARGO_REGISTRY_TOKEN when set
#[test]
fn doctor_detects_token_when_set() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    temp_env::with_vars(
        [("CARGO_REGISTRY_TOKEN", Some("secret-test-token"))],
        || {
            shipper_cmd()
                .arg("--manifest-path")
                .arg(td.path().join("Cargo.toml"))
                .arg("--api-base")
                .arg(&registry.base_url)
                .arg("doctor")
                .env("CARGO_HOME", td.path().join("cargo-home"))
                .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
                .assert()
                .success()
                .stdout(contains("auth_type: token (detected)"));
        },
    );

    registry.join();
}

/// 4. Doctor reports missing token when not set
#[test]
fn doctor_reports_missing_token() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    temp_env::with_vars(
        [
            ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
        ],
        || {
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
        },
    );

    registry.join();
}

/// 5. Doctor shows workspace info when run in a workspace
#[test]
fn doctor_shows_workspace_info() {
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
        .stdout(contains("workspace_root:"))
        .stdout(contains("registry:"))
        .stdout(contains("state_dir:"));

    registry.join();
}

/// 6. Doctor reports .shipper directory status (not yet created)
#[test]
fn doctor_reports_shipper_directory_status() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // Do NOT create .shipper dir so doctor reports it as missing
    assert!(!td.path().join(".shipper").exists());

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
        .stdout(contains("state_dir_exists: false (will be created)"));

    registry.join();
}

/// 7. Doctor reports state file if present (.shipper dir exists with state.json)
#[test]
fn doctor_reports_state_file_if_present() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    // Create .shipper directory with a state.json file
    let shipper_dir = td.path().join(".shipper");
    fs::create_dir_all(&shipper_dir).expect("mkdir .shipper");
    fs::write(
        shipper_dir.join("state.json"),
        r#"{"state_version":"shipper.state.v1"}"#,
    )
    .expect("write state.json");

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
        .stdout(contains("state_dir:"))
        .stdout(contains("state_dir_writable: true"));

    registry.join();
}
