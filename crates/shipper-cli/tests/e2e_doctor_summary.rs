//! Process-level proof for the `shipper doctor` readiness summary.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).expect("write fixture");
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

fn run_command(root: &Path, program: &str, args: &[&str]) {
    let output = StdCommand::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("run {program}: {error}"));
    assert!(
        output.status.success(),
        "{program} {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn initialize_clean_git_workspace(root: &Path) {
    run_command(root, "cargo", &["generate-lockfile"]);
    run_command(root, "git", &["init"]);
    run_command(root, "git", &["add", "."]);
    run_command(
        root,
        "git",
        &[
            "-c",
            "user.name=Shipper Test",
            "-c",
            "user.email=shipper-test@example.invalid",
            "commit",
            "-m",
            "fixture",
        ],
    );
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
        self.handle.join().expect("join registry server");
    }
}

fn spawn_registry(expected_requests: usize) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("registry server");
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        for _ in 0..expected_requests {
            let request = server
                .recv_timeout(Duration::from_secs(30))
                .expect("receive registry request")
                .expect("expected registry request");
            let response = Response::from_string(r#"{"crate":{"id":"serde"}}"#)
                .with_status_code(StatusCode(200))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json")
                        .expect("content-type header"),
                );
            request
                .respond(response)
                .expect("respond to registry request");
        }
    });
    TestRegistry { base_url, handle }
}

fn configured_doctor_command(workspace: &Path, cargo_home: &Path, registry: &str) -> Command {
    let mut command = shipper_cmd();
    command
        .arg("--allow-loopback")
        .arg("--manifest-path")
        .arg(workspace.join("Cargo.toml"))
        .arg("--api-base")
        .arg(registry)
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_REGISTRY_TOKEN", "doctor-summary-secret")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
    command
}

#[test]
#[serial_test::serial]
fn doctor_ready_summary_is_consistent_in_human_and_json_output() {
    let workspace = tempdir().expect("workspace tempdir");
    let cargo_home = tempdir().expect("cargo home tempdir");
    create_workspace(workspace.path());
    initialize_clean_git_workspace(workspace.path());
    // A ready fixture needs an existing directory whose effective access
    // Doctor can prove; a missing path correctly remains unknown.
    fs::create_dir(workspace.path().join(".shipper")).expect("create usable state directory");
    let registry = spawn_registry(2);

    configured_doctor_command(workspace.path(), cargo_home.path(), &registry.base_url)
        .arg("doctor")
        .assert()
        .success()
        .stdout(contains("Doctor: ready"))
        .stdout(contains(
            "Checks: 8 evaluated, 8 passed, 0 warnings, 0 blockers, 0 unknown",
        ))
        .stdout(contains("Next: shipper plan"))
        .stdout(predicates::str::contains("doctor-summary-secret").not());

    let output = configured_doctor_command(workspace.path(), cargo_home.path(), &registry.base_url)
        .arg("--format")
        .arg("json")
        .arg("doctor")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("doctor JSON is utf8");
    assert!(!stdout.contains("doctor-summary-secret"), "{stdout}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid doctor JSON");
    assert_eq!(
        json.pointer("/summary/readiness")
            .and_then(serde_json::Value::as_str),
        Some("ready")
    );
    assert_eq!(
        json.pointer("/summary/checks_evaluated")
            .and_then(serde_json::Value::as_u64),
        Some(8)
    );
    assert_eq!(
        json.pointer("/summary/checks_passed")
            .and_then(serde_json::Value::as_u64),
        Some(8)
    );
    assert_eq!(
        json.pointer("/summary/next_action/kind")
            .and_then(serde_json::Value::as_str),
        Some("plan")
    );
    assert_eq!(
        json.pointer("/summary/next_action/command/0")
            .and_then(serde_json::Value::as_str),
        Some("shipper")
    );
    assert_eq!(
        json.pointer("/summary/next_action/command/1")
            .and_then(serde_json::Value::as_str),
        Some("plan")
    );
    assert_eq!(
        json.pointer("/reports/0/summary/readiness")
            .and_then(serde_json::Value::as_str),
        Some("ready")
    );

    registry.join();
}
