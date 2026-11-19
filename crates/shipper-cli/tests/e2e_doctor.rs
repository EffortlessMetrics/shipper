//! End-to-end tests for the `shipper doctor` command.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
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
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

fn loopback_shipper_cmd() -> Command {
    let mut command = shipper_cmd();
    command.arg("--allow-loopback");
    command
}

fn init_git_repo(root: &Path) {
    let output = StdCommand::new("git")
        .arg("init")
        .current_dir(root)
        .output()
        .expect("git init");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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

    loopback_shipper_cmd()
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

    let output = loopback_shipper_cmd()
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
            loopback_shipper_cmd()
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

#[test]
fn doctor_json_format_reports_diagnostics_without_token_value() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    let output = loopback_shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--format")
        .arg("json")
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env("CARGO_REGISTRY_TOKEN", "secret-test-token")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    assert!(
        !stdout.contains("secret-test-token"),
        "doctor JSON must not expose token values: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(
        json.pointer("/schema_version")
            .and_then(serde_json::Value::as_str),
        Some("shipper.doctor.v1")
    );
    assert_eq!(
        json.pointer("/reports/0/registry/name")
            .and_then(serde_json::Value::as_str),
        Some("crates-io")
    );
    assert_eq!(
        json.pointer("/reports/0/auth/auth_type")
            .and_then(serde_json::Value::as_str),
        Some("token (detected)")
    );
    assert_eq!(
        json.pointer("/reports/0/connectivity/registry_reachable")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert!(
        json.pointer("/reports/0/tools/0/version")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );

    registry.join();
}

#[test]
fn doctor_json_format_redacts_registry_url_secrets() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let dead_port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    let api_base = format!(
        "http://user:url-user-secret@127.0.0.1:{dead_port}/api?token=url-token-secret&api_key=url-key-secret&scope=all"
    );

    let output = loopback_shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&api_base)
        .arg("--format")
        .arg("json")
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
    assert!(!stdout.contains("url-user-secret"), "{stdout}");
    assert!(!stdout.contains("url-token-secret"), "{stdout}");
    assert!(!stdout.contains("url-key-secret"), "{stdout}");
    assert!(stdout.contains("[REDACTED]"), "{stdout}");

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let api_base = json
        .pointer("/reports/0/registry/api_base")
        .and_then(serde_json::Value::as_str)
        .expect("api_base");
    assert!(!api_base.contains("url-token-secret"));
    assert!(!api_base.contains("url-key-secret"));

    let error = json
        .pointer("/reports/0/connectivity/registry_error")
        .and_then(serde_json::Value::as_str)
        .expect("registry error");
    assert!(!error.contains("url-token-secret"));
    assert!(!error.contains("url-key-secret"));
}

#[test]
fn doctor_json_reports_invalid_configured_registry_without_aborting() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    write_file(
        &td.path().join(".shipper.toml"),
        r#"schema_version = "shipper.config.v1"

[registry]
name = "unsafe-test"
api_base = "http://user:config-secret@127.0.0.1:9/api?token=url-secret"
"#,
    );

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--format")
        .arg("json")
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
    assert!(!stdout.contains("config-secret"), "{stdout}");
    assert!(!stdout.contains("url-secret"), "{stdout}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(
        json.pointer("/reports/0/registry/name")
            .and_then(serde_json::Value::as_str),
        Some("unsafe-test")
    );
    assert!(
        json.pointer("/reports/0/findings")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|findings| {
                findings.iter().any(|finding| {
                    finding.get("id").and_then(serde_json::Value::as_str)
                        == Some("registry-unreachable")
                })
            }),
        "{stdout}"
    );
    assert_eq!(
        json.pointer("/reports/0/connectivity/registry_reachable")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
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
            loopback_shipper_cmd()
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

#[test]
fn doctor_warns_but_does_not_pass_missing_auth_for_explicit_loopback_rehearsal() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let state_dir = td.path().join("doctor-state");
    let registry = spawn_registry(1);

    let output = loopback_shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--registry")
        .arg("isolated-rehearsal")
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_ISOLATED_REHEARSAL_TOKEN")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stdout.contains("[warning] registry auth is not proven for this loopback rehearsal"),
        "status={:?}\nstdout={stdout}\nstderr={stderr}",
        output.status.code()
    );
    assert!(
        stdout.contains("auth_type: NONE FOUND (selected-registry token missing)"),
        "{stdout}"
    );
    assert!(stdout.contains("live_auth_proven: false"), "{stdout}");
    assert!(!stdout.contains("crates.io auth is missing"), "{stdout}");
    assert!(!stdout.contains("[passed] registry auth"), "{stdout}");
    assert!(!stdout.contains("cargo login"), "{stdout}");
    assert!(!stdout.contains("crates-io-auth-action"), "{stdout}");
    assert!(
        !state_dir.exists(),
        "doctor must not create state or release evidence at {}",
        state_dir.display()
    );

    registry.join();
}

#[test]
fn doctor_json_matches_human_loopback_auth_warning_without_exposing_other_tokens() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let state_dir = td.path().join("doctor-json-state");
    let registry = spawn_registry(1);
    let secret = "unrelated-secret-sentinel";

    let output = loopback_shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--registry")
        .arg("isolated-rehearsal")
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--format")
        .arg("json")
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env("CARGO_REGISTRIES_UNRELATED_TOKEN", secret)
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_ISOLATED_REHEARSAL_TOKEN")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(!stdout.contains(secret), "stdout leaked token: {stdout}");
    assert!(!stderr.contains(secret), "stderr leaked token: {stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON: {error}; status={:?}; stdout={stdout}; stderr={stderr}",
            output.status.code()
        )
    });
    let auth_findings = json
        .pointer("/reports/0/auth/findings")
        .and_then(serde_json::Value::as_array)
        .expect("auth findings array");
    let finding = auth_findings
        .iter()
        .find(|finding| {
            finding.get("id").and_then(serde_json::Value::as_str)
                == Some("registry-auth-not-proven")
        })
        .expect("loopback auth warning");
    assert_eq!(
        finding.get("status").and_then(serde_json::Value::as_str),
        Some("warning")
    );
    assert_eq!(
        finding.get("severity").and_then(serde_json::Value::as_str),
        Some("warning")
    );
    let next_steps = finding
        .get("try_next")
        .and_then(serde_json::Value::as_array)
        .expect("try_next array");
    assert!(
        next_steps.iter().all(|step| {
            step.as_str().is_some_and(|step| {
                !step.contains("cargo login") && !step.contains("crates-io-auth-action")
            })
        }),
        "{stdout}"
    );
    assert!(
        finding
            .get("evidence")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|evidence| evidence.contains("live_auth_proven: false")),
        "{stdout}"
    );
    assert!(
        !state_dir.exists(),
        "doctor must not create state or release evidence at {}",
        state_dir.display()
    );

    registry.join();
}

#[test]
fn doctor_uses_selected_registry_token_without_exposing_it() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(1);
    let secret = "selected-registry-secret-sentinel";
    let state_dir = td.path().join("doctor-token-state");

    let output = loopback_shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--registry")
        .arg("isolated-rehearsal")
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--format")
        .arg("json")
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env("CARGO_REGISTRIES_ISOLATED_REHEARSAL_TOKEN", secret)
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(!stdout.contains(secret), "stdout leaked token: {stdout}");
    assert!(!stderr.contains(secret), "stderr leaked token: {stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid doctor JSON");
    assert_eq!(
        json.pointer("/reports/0/auth/auth_type")
            .and_then(serde_json::Value::as_str),
        Some("token (detected)")
    );
    assert!(
        json.pointer("/reports/0/auth/findings")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty),
        "{stdout}"
    );
    assert!(
        !state_dir.exists(),
        "unexpected state: {}",
        state_dir.display()
    );

    registry.join();
}

#[test]
fn doctor_keeps_non_crates_oidc_postures_blocked_and_registry_neutral() {
    for (case, oidc_url, oidc_token, expected_id, expected_auth_type) in [
        (
            "complete",
            Some("https://oidc.example.test/request"),
            Some("oidc-secret-sentinel-complete"),
            "selected-registry-auth-not-proven",
            "selected-registry auth unproven (OIDC environment detected)",
        ),
        (
            "partial",
            Some("https://oidc.example.test/request"),
            None,
            "selected-registry-auth-environment-incomplete",
            "selected-registry auth unproven (incomplete environment)",
        ),
    ] {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        write_file(
            &td.path().join(".github/workflows/release.yml"),
            "# unrelated crates.io release authority\n# rust-lang/crates-io-auth-action@v1\n",
        );
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
        let registry = spawn_registry(1);
        let state_dir = td.path().join(format!("doctor-oidc-{case}-state"));
        let mut command = loopback_shipper_cmd();
        command
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--registry")
            .arg("isolated-rehearsal")
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--format")
            .arg("json")
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_ISOLATED_REHEARSAL_TOKEN");
        match oidc_url {
            Some(value) => {
                command.env("ACTIONS_ID_TOKEN_REQUEST_URL", value);
            }
            None => {
                command.env_remove("ACTIONS_ID_TOKEN_REQUEST_URL");
            }
        }
        match oidc_token {
            Some(value) => {
                command.env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", value);
            }
            None => {
                command.env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
            }
        }

        let output = command.assert().success().get_output().clone();
        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
        assert!(!stdout.contains("crates.io"), "case={case}: {stdout}");
        assert!(
            !stdout.contains("crates-io-auth-action"),
            "case={case}: {stdout}"
        );
        assert!(!stdout.contains("cargo login"), "case={case}: {stdout}");
        if let Some(secret) = oidc_token {
            assert!(!stdout.contains(secret), "case={case}: {stdout}");
            assert!(!stderr.contains(secret), "case={case}: {stderr}");
        }
        let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid doctor JSON");
        assert_eq!(
            json.pointer("/reports/0/auth/auth_type")
                .and_then(serde_json::Value::as_str),
            Some(expected_auth_type),
            "case={case}: {stdout}"
        );
        let auth_findings = json
            .pointer("/reports/0/auth/findings")
            .and_then(serde_json::Value::as_array)
            .expect("auth findings array");
        assert!(
            auth_findings.iter().any(|finding| {
                finding.get("id").and_then(serde_json::Value::as_str) == Some(expected_id)
                    && finding.get("status").and_then(serde_json::Value::as_str) == Some("blocked")
            }),
            "case={case}: {stdout}"
        );
        assert!(
            !state_dir.exists(),
            "case={case}: unexpected state at {}",
            state_dir.display()
        );
        registry.join();

        let human_registry = spawn_registry(1);
        let human_state_dir = td.path().join(format!("doctor-oidc-{case}-human-state"));
        let mut human = loopback_shipper_cmd();
        human
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--registry")
            .arg("isolated-rehearsal")
            .arg("--api-base")
            .arg(&human_registry.base_url)
            .arg("--state-dir")
            .arg(&human_state_dir)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_ISOLATED_REHEARSAL_TOKEN");
        match oidc_url {
            Some(value) => {
                human.env("ACTIONS_ID_TOKEN_REQUEST_URL", value);
            }
            None => {
                human.env_remove("ACTIONS_ID_TOKEN_REQUEST_URL");
            }
        }
        match oidc_token {
            Some(value) => {
                human.env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", value);
            }
            None => {
                human.env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
            }
        }
        let human_output = human.assert().success().get_output().clone();
        let human_stdout = String::from_utf8(human_output.stdout).expect("utf8 human stdout");
        let human_stderr = String::from_utf8(human_output.stderr).expect("utf8 human stderr");
        assert!(
            human_stdout.contains(&format!("auth_type: {expected_auth_type}")),
            "case={case}: {human_stdout}"
        );
        assert!(
            !human_stdout.contains("trusted (detected)"),
            "case={case}: {human_stdout}"
        );
        assert!(
            !human_stdout.contains("crates.io"),
            "case={case}: {human_stdout}"
        );
        if let Some(secret) = oidc_token {
            assert!(
                !human_stdout.contains(secret),
                "case={case}: {human_stdout}"
            );
            assert!(
                !human_stderr.contains(secret),
                "case={case}: {human_stderr}"
            );
        }
        assert!(
            !human_state_dir.exists(),
            "case={case}: unexpected state at {}",
            human_state_dir.display()
        );
        human_registry.join();
    }
}

#[test]
fn doctor_does_not_infer_rehearsal_auth_from_a_loopback_endpoint_or_name() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let endpoint = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().expect("address").port()
    );
    drop(listener);
    let state_dir = td.path().join("doctor-missing-posture-state");

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--registry")
        .arg("looks-like-a-rehearsal")
        .arg("--api-base")
        .arg(endpoint)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--format")
        .arg("json")
        .arg("doctor")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_LOOKS_LIKE_A_REHEARSAL_TOKEN")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid doctor JSON");
    let auth_findings = json
        .pointer("/reports/0/auth/findings")
        .and_then(serde_json::Value::as_array)
        .expect("auth findings array");
    assert!(
        auth_findings.iter().any(|finding| {
            finding.get("id").and_then(serde_json::Value::as_str) == Some("registry-auth-missing")
                && finding.get("status").and_then(serde_json::Value::as_str) == Some("blocked")
        }),
        "{stdout}"
    );
    assert!(
        auth_findings.iter().all(|finding| {
            finding.get("id").and_then(serde_json::Value::as_str)
                != Some("registry-auth-not-proven")
        }),
        "{stdout}"
    );
    assert!(
        !state_dir.exists(),
        "unexpected state: {}",
        state_dir.display()
    );
}

/// 5. Doctor shows workspace info when run in a workspace
#[test]
fn doctor_shows_workspace_info() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    loopback_shipper_cmd()
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
#[serial_test::serial]
fn doctor_reports_shipper_directory_status() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // Do NOT create .shipper dir so doctor reports it as missing
    assert!(!td.path().join(".shipper").exists());

    let registry = spawn_registry(1);

    loopback_shipper_cmd()
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
        .stdout(contains(
            "state_dir_exists: false (prospective; write access is unverified)",
        ))
        .stdout(contains("state-dir-not-created"))
        .stdout(contains("Unknown: state directory"));

    registry.join();
    assert!(!td.path().join(".shipper").exists());
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

    loopback_shipper_cmd()
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

/// 8. Doctor turns dirty git state into an actionable finding.
#[test]
fn doctor_reports_dirty_git_remediation() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    init_git_repo(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    let registry = spawn_registry(1);

    loopback_shipper_cmd()
        .current_dir(td.path())
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
        .stdout(contains("git_dirty: true"))
        .stdout(contains(
            "[blocked] git working tree is dirty (git-working-tree-dirty)",
        ))
        .stdout(contains(
            "why: release evidence must describe the exact source tree being planned, proven, published, and resumed",
        ))
        .stdout(contains(
            "- commit, stash, or revert unrelated changes before release",
        ))
        .stdout(contains(
            "- use `--allow-dirty` only for intentional local rehearsal",
        ));

    registry.join();
}
