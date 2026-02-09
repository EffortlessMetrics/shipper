use std::env;
use std::fs;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
use insta::assert_snapshot;
use predicates::str::contains;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

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

fn normalize_output(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            if line.starts_with("plan_id: ") {
                "plan_id: <PLAN_ID>".to_string()
            } else if line.starts_with("workspace_root: ") {
                "workspace_root: <WORKSPACE_ROOT>".to_string()
            } else if line.starts_with("state_dir: ") {
                "state_dir: <STATE_DIR>".to_string()
            } else if line.starts_with("state:   ") {
                "state:   <STATE_FILE>".to_string()
            } else if line.starts_with("receipt: ") {
                "receipt: <RECEIPT_FILE>".to_string()
            } else if line.starts_with("cargo: ") {
                "cargo: <CARGO_VERSION>".to_string()
            } else if line.starts_with("git: ") {
                "git: <GIT_VERSION>".to_string()
            } else {
                line.replace('\\', "/")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn path_sep() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
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
        fs::set_permissions(path, perms).expect("chmod");
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
            let req = server.recv().expect("request");
            assert_eq!(req.url(), "/api/v1/crates/demo/0.1.0");
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

#[test]
fn plan_command_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let mut cmd = shipper_cmd();
    let out = cmd
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).expect("utf8");
    assert_snapshot!(
        normalize_output(&stdout),
        @r#"
plan_id: <PLAN_ID>
registry: crates-io (https://crates.io)
workspace_root: <WORKSPACE_ROOT>

  1. demo@0.1.0
"#
    );
}

#[test]
fn plan_command_with_package_flag() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let mut cmd = shipper_cmd();
    let out = cmd
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("demo")
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).expect("utf8");
    assert!(stdout.contains("demo@0.1.0"));
}

#[test]
fn doctor_command_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let mut cmd = shipper_cmd();
    let out = cmd
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).expect("utf8");
    assert_snapshot!(
        normalize_output(&stdout),
        @r#"
workspace_root: <WORKSPACE_ROOT>
registry: crates-io (https://crates.io)
token_detected: false
state_dir: <STATE_DIR>

cargo: <CARGO_VERSION>
git: <GIT_VERSION>
"#
    );
}

#[test]
fn status_command_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let registry = spawn_registry(vec![404], 1);

    let mut cmd = shipper_cmd();
    let out = cmd
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

    let stdout = String::from_utf8(out).expect("utf8");
    assert_snapshot!(
        normalize_output(&stdout),
        @r#"
plan_id: <PLAN_ID>

demo@0.1.0: missing
"#
    );
    registry.join();
}

#[test]
fn preflight_command_snapshot() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 1);

    let mut cmd = shipper_cmd();
    let out = cmd
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("preflight")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).expect("utf8");
    assert_snapshot!(
        normalize_output(&stdout),
        @r#"
plan_id: <PLAN_ID>
token_detected: false

demo@0.1.0: needs publish
"#
    );
    registry.join();
}

#[test]
fn preflight_command_reports_already_published() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![200], 1);

    let mut cmd = shipper_cmd();
    let out = cmd
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("preflight")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).expect("utf8");
    assert!(stdout.contains("already published"));
    registry.join();
}

#[test]
fn publish_command_e2e_with_fake_cargo() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let bin_dir = td.path().join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("mkdir");
    create_fake_cargo_proxy(&bin_dir);

    let old_path = env::var("PATH").unwrap_or_default();
    let mut new_path = bin_dir.display().to_string();
    if !old_path.is_empty() {
        new_path.push_str(path_sep());
        new_path.push_str(&old_path);
    }
    let real_cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    let registry = spawn_registry(vec![404, 200], 2);

    let mut cmd = shipper_cmd();
    let out = cmd
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
        .arg(".shipper")
        .arg("publish")
        .env("PATH", new_path)
        .env("REAL_CARGO", real_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).expect("utf8");
    assert!(stdout.contains("demo@0.1.0: Published"));
    assert!(td.path().join(".shipper").join("state.json").exists());
    assert!(td.path().join(".shipper").join("receipt.json").exists());
    registry.join();
}

#[test]
fn publish_then_resume_e2e_with_absolute_state_dir() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let bin_dir = td.path().join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("mkdir");
    create_fake_cargo_proxy(&bin_dir);

    let old_path = env::var("PATH").unwrap_or_default();
    let mut new_path = bin_dir.display().to_string();
    if !old_path.is_empty() {
        new_path.push_str(path_sep());
        new_path.push_str(&old_path);
    }
    let real_cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let abs_state = td.path().join("shipper-state-abs");

    let registry = spawn_registry(vec![404, 200], 2);

    let mut publish = shipper_cmd();
    publish
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
        .arg(&abs_state)
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success();
    registry.join();

    let mut resume = shipper_cmd();
    let out = resume
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:9")
        .arg("--allow-dirty")
        .arg("--force-resume")
        .arg("--state-dir")
        .arg(&abs_state)
        .arg("resume")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(out).expect("utf8");
    assert!(stdout.contains("plan_id:"));
    assert!(stdout.contains("state:"));
    assert!(stdout.contains("receipt:"));
}

#[test]
fn invalid_duration_flag_fails() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--base-delay")
        .arg("not-a-duration")
        .arg("plan")
        .assert()
        .failure()
        .stderr(contains("invalid duration"));
}
