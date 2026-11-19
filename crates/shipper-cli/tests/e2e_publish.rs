//! End-to-end tests for the full `shipper publish` flow.
//!
//! Tests cover single-crate and multi-crate publishes, state/receipt/events
//! verification, --dry-run-like behavior, --package scoping, custom --state-dir,
//! failed publishes, and re-running publish when everything is already published.

#[path = "e2e_publish/ci_github_actions.rs"]
mod ci_github_actions;
#[path = "e2e_publish/live_retry.rs"]
mod live_retry;
#[path = "e2e_publish/lock_admission.rs"]
mod lock_admission;
#[path = "e2e_publish/per_error_refusal.rs"]
mod per_error_refusal;
#[path = "e2e_publish/receipt_git_context.rs"]
mod receipt_git_context;
#[path = "e2e_publish/registry_failure.rs"]
mod registry_failure;
#[path = "e2e_publish/resume_scope.rs"]
mod resume_scope;
#[path = "e2e_publish/retained_admission.rs"]
mod retained_admission;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};
use assert_cmd::Command;
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

fn create_independent_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"alpha\", \"beta\"]\nresolver = \"2\"\n",
    );
    for name in ["alpha", "beta"] {
        write_file(
            &root.join(name).join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write_file(
            &root.join(name).join("src/lib.rs"),
            &format!("pub fn {name}() {{}}\n"),
        );
    }
}

fn create_fake_cargo_proxy(bin_dir: &Path) {
    #[cfg(windows)]
    {
        fs::write(
            bin_dir.join("cargo.cmd"),
            "@echo off\r\nif \"%1\"==\"publish\" (\r\n  if not \"%SHIPPER_FAKE_PUBLISH_LOG%\"==\"\" echo %*>>\"%SHIPPER_FAKE_PUBLISH_LOG%\"\r\n  if not \"%SHIPPER_FAKE_RECONCILIATION_DIR%\"==\"\" (\r\n    if not exist \"%SHIPPER_FAKE_RECONCILIATION_DIR%\" mkdir \"%SHIPPER_FAKE_RECONCILIATION_DIR%\"\r\n    if not exist \"%SHIPPER_FAKE_RECONCILIATION_DIR%\" exit /b 91\r\n  )\r\n  if not \"%SHIPPER_FAKE_PUBLISH_STDERR%\"==\"\" echo %SHIPPER_FAKE_PUBLISH_STDERR% 1>&2\r\n  if \"%SHIPPER_FAKE_PUBLISH_EXIT%\"==\"\" (exit /b 0) else (exit /b %SHIPPER_FAKE_PUBLISH_EXIT%)\r\n)\r\n\"%REAL_CARGO%\" %*\r\nexit /b %ERRORLEVEL%\r\n",
        )
        .expect("write fake cargo");
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cargo");
        fs::write(
            &path,
            "#!/usr/bin/env sh\nif [ \"$1\" = \"publish\" ]; then\n  if [ -n \"$SHIPPER_FAKE_PUBLISH_LOG\" ]; then\n    printf '%s\\n' \"$*\" >> \"$SHIPPER_FAKE_PUBLISH_LOG\"\n  fi\n  if [ -n \"$SHIPPER_FAKE_RECONCILIATION_DIR\" ]; then\n    mkdir -p \"$SHIPPER_FAKE_RECONCILIATION_DIR\" || exit 91\n  fi\n  if [ -n \"$SHIPPER_FAKE_PUBLISH_STDERR\" ]; then\n    printf '%s\\n' \"$SHIPPER_FAKE_PUBLISH_STDERR\" >&2\n  fi\n  exit \"${SHIPPER_FAKE_PUBLISH_EXIT:-0}\"\nfi\n\"$REAL_CARGO\" \"$@\"\n",
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

struct BoundedTestRegistry {
    base_url: String,
    server: Arc<Server>,
    handle: thread::JoinHandle<usize>,
    completed: mpsc::Receiver<usize>,
    expected_requests: usize,
}

impl BoundedTestRegistry {
    fn finish(self, timeout: Duration) -> Result<()> {
        let observed = match self.completed.recv_timeout(timeout) {
            Ok(observed) => observed,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.server.unblock();
                let observed = self
                    .handle
                    .join()
                    .map_err(|_| anyhow!("mock registry thread panicked after deadline"))?;
                bail!(
                    "mock registry deadline elapsed: expected {} requests, observed {observed}",
                    self.expected_requests
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("mock registry completion channel disconnected");
            }
        };
        let joined = self
            .handle
            .join()
            .map_err(|_| anyhow!("mock registry thread panicked"))?;
        if joined != observed {
            bail!(
                "mock registry completion mismatch: channel reported {observed}, thread returned {joined}"
            );
        }
        if observed != self.expected_requests {
            bail!(
                "mock registry request mismatch: expected {}, observed {observed}",
                self.expected_requests
            );
        }
        Ok(())
    }
}

fn spawn_bounded_registry(statuses: Vec<u16>, expected_requests: usize) -> BoundedTestRegistry {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base_url = format!("http://{}", server.server_addr());
    let worker_server = Arc::clone(&server);
    let (completed_tx, completed) = mpsc::sync_channel(1);
    let handle = thread::spawn(move || {
        let mut observed = 0;
        for idx in 0..expected_requests {
            let req = match worker_server.recv_timeout(Duration::from_secs(30)) {
                Ok(Some(request)) => request,
                _ => break,
            };
            observed += 1;
            let status = statuses
                .get(idx)
                .copied()
                .or_else(|| statuses.last().copied())
                .unwrap_or(404);
            let response = Response::from_string("{}")
                .with_status_code(StatusCode(status))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json").expect("header"),
                );
            req.respond(response).expect("respond");
        }
        if let Ok(Some(request)) = worker_server.recv_timeout(Duration::from_millis(500)) {
            observed += 1;
            let status = statuses.last().copied().unwrap_or(404);
            let response = Response::from_string("{}")
                .with_status_code(StatusCode(status))
                .with_header(
                    Header::from_bytes("Content-Type", "application/json").expect("header"),
                );
            request.respond(response).expect("respond to extra request");
        }
        let _ = completed_tx.send(observed);
        observed
    });
    BoundedTestRegistry {
        base_url,
        server,
        handle,
        completed,
        expected_requests,
    }
}

fn send_registry_request(base_url: &str) -> Result<()> {
    let address = base_url
        .strip_prefix("http://")
        .context("mock registry URL must use http")?;
    let socket = address
        .to_socket_addrs()
        .context("resolve mock registry address")?
        .next()
        .context("mock registry address must resolve")?;
    let io_timeout = Duration::from_millis(500);
    let mut stream = TcpStream::connect_timeout(&socket, io_timeout)
        .context("connect to mock registry within deadline")?;
    stream
        .set_read_timeout(Some(io_timeout))
        .context("set mock registry read deadline")?;
    stream
        .set_write_timeout(Some(io_timeout))
        .context("set mock registry write deadline")?;
    stream
        .write_all(b"GET /api/v1/crates/demo/0.1.0 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .context("write mock registry request")?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .context("read mock registry response")?;
    if !response.starts_with(b"HTTP/1.1 200") {
        bail!(
            "mock registry returned unexpected response: {}",
            String::from_utf8_lossy(&response)
        );
    }
    Ok(())
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

fn loopback_shipper_cmd() -> Command {
    let mut command = shipper_cmd();
    command.arg("--allow-loopback");
    command
}

fn assert_publish_early_error_json(
    output: &std::process::Output,
    expected_category: &str,
    state_dir: &Path,
) -> serde_json::Value {
    assert_eq!(output.status.code(), Some(1), "early publish failure exit");
    assert!(
        output.stdout.is_empty(),
        "JSON errors keep stdout empty; got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr utf8");
    assert_secret_absent_from_output_and_state(output, state_dir);
    let report: serde_json::Value =
        serde_json::from_str(&stderr).expect("stderr should be one JSON error envelope");
    assert_eq!(
        report["schema_version"].as_str(),
        Some("shipper.publish.error.v1")
    );
    assert_eq!(report["command"].as_str(), Some("publish"));
    assert_eq!(report["status"].as_str(), Some("failed"));
    assert_eq!(report["category"].as_str(), Some(expected_category));
    assert!(report["summary"].is_string());
    assert!(report["safe_to_rerun"]["value"].is_null());
    assert_eq!(
        report["safe_to_rerun"]["reason"].as_str(),
        Some("no completed receipt exists to prove a safe rerun")
    );
    assert!(report["next_action"]["command"].is_null());
    assert_eq!(
        report["next_action"]["requires_confirmation"].as_bool(),
        Some(false)
    );
    assert_eq!(report["evidence"], serde_json::json!([]));
    report
}

fn assert_secret_absent_from_output_and_state(output: &std::process::Output, state_dir: &Path) {
    assert_sentinel_absent_from_output_and_state(output, state_dir, "EARLY_ERROR_SECRET");
}

fn assert_sentinel_absent_from_output_and_state(
    output: &std::process::Output,
    state_dir: &Path,
    sentinel: &str,
) {
    for (surface, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let rendered = String::from_utf8_lossy(bytes);
        assert!(
            !rendered.contains(sentinel),
            "secret leaked in {surface}: {rendered}"
        );
    }
    if !state_dir.exists() {
        return;
    }
    let mut pending = vec![state_dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path).expect("read state directory") {
            let entry = entry.expect("state entry");
            let entry_path = entry.path();
            if entry_path.is_dir() {
                pending.push(entry_path);
                continue;
            }
            let Some(name) = entry_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if matches!(
                name,
                "state.json" | "events.jsonl" | "receipt.json" | "reconciliation.json"
            ) {
                let contents = fs::read(&entry_path).expect("read retained evidence");
                assert!(
                    !String::from_utf8_lossy(&contents).contains(sentinel),
                    "secret leaked in {}",
                    entry_path.display()
                );
            }
        }
    }
}

fn assert_registry_completion_artifacts(
    registry_state: &Path,
    registry: &str,
    expected_result: &str,
    expected_state: &str,
) {
    let state_path = registry_state.join("state.json");
    let events_path = registry_state.join("events.jsonl");
    let receipt_path = registry_state.join("receipt.json");
    for artifact in [&state_path, &events_path, &receipt_path] {
        assert!(
            artifact.exists(),
            "{registry} must retain {}",
            artifact.display()
        );
    }

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&state_path).expect("read registry state"))
            .expect("parse registry state");
    let receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&receipt_path).expect("read registry receipt"))
            .expect("parse registry receipt");

    assert_eq!(state["registry"]["name"].as_str(), Some(registry));
    assert_eq!(receipt["registry"]["name"].as_str(), Some(registry));
    assert_eq!(state["plan_id"], receipt["plan_id"]);
    assert_eq!(
        state["packages"]["demo@0.1.0"]["state"]["state"].as_str(),
        Some(expected_state),
        "{registry} state package result"
    );
    assert_eq!(receipt["packages"].as_array().map(Vec::len), Some(1));
    assert_eq!(receipt["packages"][0]["name"].as_str(), Some("demo"));
    assert_eq!(receipt["packages"][0]["version"].as_str(), Some("0.1.0"));
    assert_eq!(
        receipt["packages"][0]["state"]["state"].as_str(),
        Some(expected_state),
        "{registry} receipt package result"
    );
    assert_eq!(
        receipt["execution_result"].as_str(),
        Some(expected_result),
        "{registry} receipt result"
    );
    assert_eq!(
        receipt["event_log_path"].as_str(),
        Some(events_path.to_string_lossy().as_ref()),
        "{registry} receipt must identify its authoritative event log"
    );

    let finished: Vec<serde_json::Value> = fs::read_to_string(&events_path)
        .expect("read registry events")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse registry event"))
        .filter(|event: &serde_json::Value| {
            event["event_type"]["type"].as_str() == Some("execution_finished")
        })
        .collect();
    assert_eq!(finished.len(), 1, "{registry} execution-finished count");
    assert_eq!(finished[0]["package"].as_str(), Some("all"));
    assert_eq!(
        finished[0]["event_type"]["result"].as_str(),
        Some(expected_result),
        "{registry} authoritative event result"
    );
}

/// Build env vars needed for fake cargo, returning (new_path, real_cargo, fake_cargo).
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

/// Set up a temp dir with fake cargo bin and return (bin_dir, new_path, real_cargo, fake_cargo).
fn setup_fake_cargo(td: &Path) -> (String, String, String) {
    let bin_dir = td.join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("mkdir");
    create_fake_cargo_proxy(&bin_dir);
    fake_cargo_env(&bin_dir)
}

fn read_publish_log(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }

    fs::read_to_string(path)
        .expect("read publish log")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn command_package_state<'a>(packages: &'a [serde_json::Value], name: &str) -> &'a str {
    packages
        .iter()
        .find(|package| package["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("expected command package report for {name}"))["state"]
        .as_str()
        .unwrap_or_else(|| panic!("expected command package state for {name}"))
}

fn receipt_package_state<'a>(packages: &'a [serde_json::Value], name: &str) -> &'a str {
    packages
        .iter()
        .find(|package| package["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("expected receipt package for {name}"))["state"]["state"]
        .as_str()
        .unwrap_or_else(|| panic!("expected receipt package state for {name}"))
}

// ============================================================================
// Test 1: Single-crate publish success with state/receipt verification
// ============================================================================

#[test]
fn single_crate_publish_creates_state_and_receipt() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // version-check 404 (not yet published), then readiness 200 (visible)
    let registry = spawn_registry(vec![404, 200], 2);

    let state_dir = td.path().join(".shipper");

    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
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

    let stdout = String::from_utf8(output).expect("publish stdout utf8");
    assert!(stdout.contains("Result: success"), "{stdout}");
    assert!(
        stdout.contains("Safe to rerun: yes — all packages reached a successful terminal state"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Next: publication is complete; retain the receipt and event evidence"),
        "{stdout}"
    );

    // Verify state.json exists and has plan_id
    let state_path = state_dir.join("state.json");
    assert!(state_path.exists(), "state.json should exist");
    let state_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&state_path).expect("read state"))
            .expect("parse state");
    assert!(
        state_json.get("plan_id").is_some(),
        "state should have plan_id"
    );

    // Verify receipt.json exists and shows published
    let receipt_path = state_dir.join("receipt.json");
    assert!(receipt_path.exists(), "receipt.json should exist");
    let receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&receipt_path).expect("read receipt"))
            .expect("parse receipt");
    assert!(
        receipt.get("plan_id").is_some(),
        "receipt should have plan_id"
    );

    let packages = receipt["packages"].as_array().expect("packages array");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"].as_str(), Some("demo"));
    assert_eq!(
        packages[0]["state"]["state"].as_str(),
        Some("published"),
        "package should be marked published"
    );

    // Verify receipt has timing fields
    assert!(
        receipt.get("started_at").is_some(),
        "receipt should have started_at"
    );
    assert!(
        receipt.get("finished_at").is_some(),
        "receipt should have finished_at"
    );

    registry.join();
}

#[test]
fn publish_json_format_writes_command_envelope_to_stdout() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    let registry = spawn_registry(vec![404, 200], 2);
    let state_dir = td.path().join(".shipper");
    let state_dir_arg = Path::new(".shipper");

    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
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
        .arg(state_dir_arg)
        .arg("--format")
        .arg("json")
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
    let report: serde_json::Value =
        serde_json::from_str(&stdout).expect("publish stdout should be command JSON");
    assert_eq!(
        report["schema_version"].as_str(),
        Some("shipper.publish.v1"),
        "publish JSON should carry a command-owned schema version"
    );
    assert_eq!(report["command"].as_str(), Some("publish"));
    assert_eq!(
        report["safe_to_rerun"].as_bool(),
        Some(true),
        "completed publish envelope should expose safe rerun posture"
    );
    assert_eq!(report["registry"].as_str(), Some("crates-io"));
    assert_eq!(
        report["state_dir"].as_str(),
        Some(state_dir_arg.to_string_lossy().as_ref()),
        "legacy state_dir must preserve the configured relative value"
    );
    assert_eq!(report["outcome"]["status"].as_str(), Some("success"));
    assert_eq!(
        report["outcome"]["safe_to_rerun"]["value"].as_bool(),
        Some(true)
    );
    assert_eq!(
        report["safe_to_rerun"], report["outcome"]["safe_to_rerun"]["value"],
        "legacy and typed rerun fields must share one computed value"
    );
    assert_eq!(
        report["outcome"]["next_action"]["kind"].as_str(),
        Some("none_complete")
    );
    let evidence = report["outcome"]["evidence"]
        .as_array()
        .expect("typed evidence array");
    let evidence = evidence
        .iter()
        .map(|path| path.as_str().expect("evidence path").to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        evidence,
        ["state.json", "events.jsonl", "receipt.json"]
            .map(|name| state_dir.join(name).to_string_lossy().into_owned()),
        "typed evidence must name the workspace-resolved state, events, and receipt files"
    );
    assert!(report["plan_id"].is_string(), "plan_id should be present");
    assert_eq!(report["published"].as_u64(), Some(1));
    assert_eq!(report["pending"].as_u64(), Some(0));
    assert_eq!(report["failed"].as_u64(), Some(0));
    assert_eq!(report["ambiguous"].as_u64(), Some(0));
    assert_eq!(report["uploaded"].as_u64(), Some(0));
    assert_eq!(report["skipped"].as_u64(), Some(0));
    assert_eq!(
        report["packages"][0]["name"].as_str(),
        Some("demo"),
        "command envelope should contain the published package"
    );
    assert_eq!(report["packages"][0]["state"].as_str(), Some("published"));
    assert_eq!(report["packages"][0]["attempts"].as_u64(), Some(1));
    assert_eq!(report["packages"][0]["reconciled"].as_bool(), Some(false));
    assert_eq!(
        report["artifacts"]["state"]["exists"].as_bool(),
        Some(true),
        "state artifact should exist"
    );
    for (artifact, file) in [
        ("state", "state.json"),
        ("events", "events.jsonl"),
        ("receipt", "receipt.json"),
    ] {
        assert_eq!(
            report["artifacts"][artifact]["path"].as_str(),
            Some(state_dir_arg.join(file).to_string_lossy().as_ref()),
            "legacy {artifact} artifact path must preserve the configured relative state directory"
        );
    }
    assert_eq!(
        report["artifacts"]["events"]["exists"].as_bool(),
        Some(true),
        "events artifact should exist"
    );
    assert_eq!(
        report["artifacts"]["receipt"]["exists"].as_bool(),
        Some(true),
        "receipt artifact should exist"
    );
    assert_eq!(
        report["artifacts"]["reconciliation"]["exists"].as_bool(),
        Some(false),
        "reconciliation artifact should be absent when no ambiguity occurred"
    );
    assert_eq!(
        report["receipt"]["receipt_version"].as_str(),
        Some("shipper.receipt.v2"),
        "receipt remains nested as its own evidence contract"
    );
    assert_eq!(
        report["receipt"]["packages"][0]["state"]["state"].as_str(),
        Some("published")
    );
    assert!(
        state_dir.join("receipt.json").exists(),
        "receipt artifact should still be written"
    );

    registry.join();
}

#[test]
fn publish_missing_and_invalid_manifests_emit_typed_json_errors() {
    let td = tempdir().expect("tempdir");
    let malformed = td.path().join("malformed.toml");
    write_file(&malformed, "[workspace\n");

    for (case, manifest) in [
        ("missing", td.path().join("missing.toml")),
        ("malformed", malformed),
    ] {
        let state_dir = td.path().join(format!("state-{case}"));
        let output = shipper_cmd()
            .timeout(Duration::from_secs(20))
            .arg("--manifest-path")
            .arg(manifest)
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--format")
            .arg("json")
            .arg("publish")
            .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET")
            .assert()
            .get_output()
            .clone();
        let report = assert_publish_early_error_json(&output, "invalid_manifest", &state_dir);
        assert_eq!(
            report["next_action"]["kind"].as_str(),
            Some("resolve_blockers")
        );
        assert!(!state_dir.exists(), "plan errors must not create state");
    }
}

#[test]
fn publish_manifest_error_human_output_matches_typed_posture() {
    let td = tempdir().expect("tempdir");
    let state_dir = td.path().join("state-human");
    let output = shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("missing.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("publish")
        .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET")
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    assert_secret_absent_from_output_and_state(&output, &state_dir);
    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr utf8");
    assert!(stderr.contains("Error:"), "{stderr}");
    assert!(
        stderr.contains("Result: failed — the publish plan could not be built"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Safe to rerun: unknown — no completed receipt exists"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Next: resolve the reported failure"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Evidence: none from a completed receipt"),
        "{stderr}"
    );
    assert!(!state_dir.exists(), "plan errors must not create state");
}

#[test]
fn publish_strict_ownership_without_token_stops_before_both_schedulers() {
    const SECRET: &str = "UNSELECTED_REGISTRY_SECRET";

    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let cargo_home = td.path().join("empty-cargo-home");
    fs::create_dir_all(&cargo_home).expect("create isolated cargo home");

    for parallel in [false, true] {
        let registry = spawn_bounded_registry(Vec::new(), 0);
        let state_dir = td.path().join(if parallel {
            "strict-parallel-state"
        } else {
            "strict-sequential-state"
        });
        let mut command = loopback_shipper_cmd();
        command
            .timeout(Duration::from_secs(20))
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--strict-ownership")
            .arg("--state-dir")
            .arg(&state_dir);
        if parallel {
            command.arg("--parallel");
        }
        let output = command
            .arg("publish")
            .env("CARGO_HOME", &cargo_home)
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .env("CARGO_REGISTRIES_UNUSED_TOKEN", SECRET)
            .assert()
            .code(1)
            .get_output()
            .clone();

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("strict ownership requested but no token found"),
            "{stderr}"
        );
        assert!(output.stdout.is_empty(), "publish failure stdout");
        assert!(
            !state_dir.exists(),
            "strict gate must precede all {parallel:?} scheduler artifacts"
        );
        assert_sentinel_absent_from_output_and_state(&output, &state_dir, SECRET);
        registry
            .finish(Duration::from_secs(2))
            .expect("strict gate must make zero registry requests");
    }
}

#[test]
fn multi_registry_strict_ownership_prevalidates_every_token_before_alpha_runs() {
    const ALPHA_TOKEN: &str = "ALPHA_TOKEN_MUST_NOT_LEAK";

    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let cargo_home = td.path().join("empty-multi-registry-cargo-home");
    fs::create_dir_all(&cargo_home).expect("create isolated cargo home");
    let alpha = spawn_bounded_registry(Vec::new(), 0);
    let beta = spawn_bounded_registry(Vec::new(), 0);
    let config = td.path().join("strict-multi-registry.toml");
    write_file(
        &config,
        &format!(
            r#"
schema_version = "shipper.config.v1"

[[registries.registries]]
name = "alpha"
api_base = "{alpha}"
index_base = "{alpha}"

[[registries.registries]]
name = "beta"
api_base = "{beta}"
index_base = "{beta}"
"#,
            alpha = alpha.base_url,
            beta = beta.base_url,
        ),
    );
    let state_dir = td.path().join("strict-multi-registry-state");

    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&config)
        .arg("--registries")
        .arg("alpha,beta")
        .arg("--allow-dirty")
        .arg("--strict-ownership")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("publish")
        .env("CARGO_HOME", &cargo_home)
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .env("CARGO_REGISTRIES_ALPHA_TOKEN", ALPHA_TOKEN)
        .env_remove("CARGO_REGISTRIES_BETA_TOKEN")
        .assert()
        .code(1)
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("strict ownership requested but no token found"),
        "{stderr}"
    );
    assert!(
        stderr.contains("configure a token for every selected registry, then rerun publish"),
        "{stderr}"
    );
    for nonexistent_evidence_hint in ["events.jsonl", "state.json", "shipper resume"] {
        assert!(
            !stderr.contains(nonexistent_evidence_hint),
            "prevalidation must not recommend nonexistent evidence: {stderr}"
        );
    }
    assert!(output.stdout.is_empty(), "publish failure stdout");
    assert!(
        !state_dir.exists(),
        "no selected registry may create state before all tokens validate"
    );
    assert_sentinel_absent_from_output_and_state(&output, &state_dir, ALPHA_TOKEN);
    alpha
        .finish(Duration::from_secs(2))
        .expect("alpha must not receive a request");
    beta.finish(Duration::from_secs(2))
        .expect("beta must not receive a request");
}

#[test]
fn publish_non_git_workspace_emits_typed_json_error() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let state_dir = td.path().join("state-non-git");
    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:9")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--quiet")
        .arg("--format")
        .arg("json")
        .arg("publish")
        .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET")
        .assert()
        .get_output()
        .clone();
    assert_publish_early_error_json(&output, "workspace_not_ready", &state_dir);
    assert!(!state_dir.join("receipt.json").exists());
}

#[test]
fn publish_unreachable_registry_emits_typed_json_error() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let state_dir = td.path().join("state-unreachable");
    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:9")
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--verify-timeout")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--quiet")
        .arg("--format")
        .arg("json")
        .arg("publish")
        .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET")
        .assert()
        .get_output()
        .clone();
    let report = assert_publish_early_error_json(&output, "registry_unreachable", &state_dir);
    assert_eq!(
        report["next_action"]["kind"].as_str(),
        Some("stop_and_investigate")
    );
    assert!(!state_dir.join("receipt.json").exists());
}

#[test]
fn publish_multi_registry_json_error_is_one_clean_envelope() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let state_dir = td.path().join("state-multi-registry");
    let config = td.path().join("multi-registry.toml");
    write_file(
        &config,
        r#"
[[registries.registries]]
name = "alpha"
api_base = "http://127.0.0.1:9"
index_base = "http://127.0.0.1:9"

[[registries.registries]]
name = "beta"
api_base = "https://example.invalid"
index_base = "https://example.invalid"

[rehearsal]
registry = "alpha"
allow_loopback = true
"#,
    );
    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&config)
        .arg("--registries")
        .arg("alpha,beta")
        .arg("--skip-ownership-check")
        .arg("--verify-timeout")
        .arg("0ms")
        .arg("--max-attempts")
        .arg("1")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--max-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--format")
        .arg("json")
        .arg("publish")
        .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET")
        .assert()
        .get_output()
        .clone();
    let report = assert_publish_early_error_json(&output, "workspace_not_ready", &state_dir);
    assert_eq!(
        report["next_action"]["kind"].as_str(),
        Some("resolve_blockers")
    );
    assert!(!state_dir.join("receipt.json").exists());
}

struct CompletedPartialRun {
    _workspace: tempfile::TempDir,
    state_dir: std::path::PathBuf,
    output: std::process::Output,
}

struct StillUnknownRun {
    _workspace: tempfile::TempDir,
    state_dir: std::path::PathBuf,
    publish_log: std::path::PathBuf,
    output: std::process::Output,
}

struct ControlledStopRun {
    _workspace: tempfile::TempDir,
    state_dir: std::path::PathBuf,
    output: std::process::Output,
}

struct ParallelInconsistentRun {
    workspace: tempfile::TempDir,
    state_dir: std::path::PathBuf,
    api_base: String,
    publish_log: std::path::PathBuf,
    new_path: String,
    real_cargo: String,
    fake_cargo: String,
    output: std::process::Output,
}

fn run_parallel_inconsistent_controlled_stop(
    format: Option<&str>,
) -> Result<ParallelInconsistentRun> {
    let td = tempdir().context("create parallel inconsistent workspace")?;
    create_independent_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let registry = spawn_bounded_registry(vec![404, 404, 404, 404], 4);
    let api_base = registry.base_url.clone();
    let state_dir = td.path().join("parallel-inconsistent-state");
    // Inject the write failure only after valid admission, inside fake Cargo.
    ensure!(!state_dir.exists());
    let publish_log = td.path().join("parallel-inconsistent.log");
    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .current_dir(td.path())
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&api_base)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--parallel")
        .arg("--max-concurrent")
        .arg("2")
        .arg("--readiness-timeout")
        .arg("0ms")
        .arg("--readiness-poll")
        .arg("0ms")
        .arg("--max-attempts")
        .arg("1")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir);
    if let Some(format) = format {
        command.arg("--format").arg(format);
    }
    let output = command
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
        .env("SHIPPER_FAKE_PUBLISH_STDERR", "ambiguous parallel close")
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .env(
            "SHIPPER_FAKE_RECONCILIATION_DIR",
            state_dir.join("reconciliation.json"),
        )
        .env("CARGO_REGISTRY_TOKEN", "CONTROLLED_STOP_SECRET_SENTINEL")
        .output()?;
    registry.finish(Duration::from_secs(2))?;
    Ok(ParallelInconsistentRun {
        workspace: td,
        state_dir,
        api_base,
        publish_log,
        new_path,
        real_cargo,
        fake_cargo,
        output,
    })
}

fn run_initial_controlled_stop_publish(
    format: Option<&str>,
    block_reconciliation_write: bool,
) -> Result<ControlledStopRun> {
    let td = tempdir().context("create initial controlled-stop workspace")?;
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let registry = spawn_bounded_registry(vec![404, 404], 2);
    let state_dir = td.path().join("initial-controlled-stop-state");
    ensure!(!state_dir.exists());
    let publish_log = td.path().join("initial-controlled-stop.log");
    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .current_dir(td.path())
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--readiness-timeout")
        .arg("0ms")
        .arg("--readiness-poll")
        .arg("0ms")
        .arg("--max-attempts")
        .arg("1")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir);
    if let Some(format) = format {
        command.arg("--format").arg(format);
    }
    // A pre-existing invalid artifact must be refused by admission. This
    // fixture instead faults the later reconciliation write after Cargo starts.
    command.env_remove("SHIPPER_FAKE_RECONCILIATION_DIR");
    if block_reconciliation_write {
        command.env(
            "SHIPPER_FAKE_RECONCILIATION_DIR",
            state_dir.join("reconciliation.json"),
        );
    }
    let output = command
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
        .env("SHIPPER_FAKE_PUBLISH_STDERR", "ambiguous transport close")
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .env("CARGO_REGISTRY_TOKEN", "CONTROLLED_STOP_SECRET_SENTINEL")
        .output()?;
    registry.finish(Duration::from_secs(2))?;
    ensure!(read_publish_log(&publish_log).len() == 1);
    Ok(ControlledStopRun {
        _workspace: td,
        state_dir,
        output,
    })
}

fn run_ambiguous_still_unknown_publish(format: Option<&str>) -> Result<StillUnknownRun> {
    let td = tempdir().context("create ambiguous publish workspace")?;
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let registry = spawn_bounded_registry(vec![500, 500], 2);
    let state_dir = td.path().join("state-ambiguous");
    let publish_log = td.path().join("publish.log");

    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--no-readiness")
        .arg("--max-attempts")
        .arg("1")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--quiet");
    if let Some(format) = format {
        command.arg("--format").arg(format);
    }
    let output = command
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
        .env("SHIPPER_FAKE_PUBLISH_STDERR", "")
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .env("CARGO_REGISTRY_TOKEN", "AMBIGUOUS_OUTCOME_SECRET")
        .output()
        .context("run ambiguous StillUnknown publish fixture")?;
    registry.finish(Duration::from_secs(2))?;

    Ok(StillUnknownRun {
        _workspace: td,
        state_dir,
        publish_log,
        output,
    })
}

fn run_completed_partial_publish(format: Option<&str>) -> Result<CompletedPartialRun> {
    let td = tempdir().context("create completed-partial workspace")?;
    create_single_crate_workspace(td.path());
    let state_dir = td.path().join(".shipper");
    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:9")
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--max-attempts")
        .arg("0")
        .arg("--quiet")
        .arg("--state-dir")
        .arg(&state_dir);
    if let Some(format) = format {
        command.arg("--format").arg(format);
    }
    let output = command
        .arg("publish")
        .env("CARGO_REGISTRY_TOKEN", "EARLY_ERROR_SECRET")
        .output()
        .context("run completed-partial publish fixture")?;

    Ok(CompletedPartialRun {
        _workspace: td,
        state_dir,
        output,
    })
}

fn human_outcome_value<'a>(stdout: &'a str, label: &str) -> Result<&'a str> {
    let values = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(label))
        .map(str::trim)
        .collect::<Vec<_>>();
    match values.as_slice() {
        [value] => Ok(value),
        [] => bail!("missing human outcome line {label:?} in:\n{stdout}"),
        _ => bail!("duplicate human outcome lines for {label:?}: {values:?} in:\n{stdout}"),
    }
}

fn normalize_state_identity(value: &str, state_dir: &Path) -> String {
    value.replace(state_dir.to_string_lossy().as_ref(), "<STATE_DIR>")
}

#[test]
fn completed_partial_publish_human_and_json_have_semantic_parity() -> Result<()> {
    let human = run_completed_partial_publish(None)?;
    let json = run_completed_partial_publish(Some("json"))?;
    ensure!(human.output.status.code() == Some(2));
    ensure!(json.output.status.code() == Some(2));
    ensure!(
        json.output.stderr.is_empty(),
        "completed JSON stays off stderr"
    );

    let human_stdout = std::str::from_utf8(&human.output.stdout).context("human publish stdout")?;
    let human_result = human_outcome_value(human_stdout, "Result:")?;
    let human_safe = human_outcome_value(human_stdout, "Safe to rerun:")?;
    let human_next = human_outcome_value(human_stdout, "Next:")?;
    let human_evidence = human_outcome_value(human_stdout, "Evidence:")?;

    let report: serde_json::Value =
        serde_json::from_slice(&json.output.stdout).context("completed-partial publish JSON")?;
    ensure!(report["schema_version"] == "shipper.publish.v1");
    ensure!(report["execution_result"] == "partial_failure");
    ensure!(report["outcome"]["status"] == "partial_failure");
    ensure!(report["safe_to_rerun"] == false);
    ensure!(report["outcome"]["safe_to_rerun"]["value"] == false);
    ensure!(report["outcome"]["next_action"]["kind"] == "resume");
    let next_action = report["outcome"]["next_action"]
        .as_object()
        .ok_or_else(|| anyhow!("missing typed next action"))?;
    ensure!(
        !next_action.contains_key("command"),
        "commandless posture must omit command: {next_action:?}"
    );
    ensure!(report["pending"] == 1);
    ensure!(report["published"] == 0);
    ensure!(report["failed"] == 0);
    ensure!(report["ambiguous"] == 0);
    ensure!(report["uploaded"] == 0);
    ensure!(report["skipped"] == 0);
    ensure!(report["packages"].as_array().map(Vec::len) == Some(1));
    ensure!(report["packages"][0]["state"] == "pending");

    ensure!(human_result == "partial failure");
    let (human_safe_value, human_safe_reason) = human_safe
        .split_once(" — ")
        .ok_or_else(|| anyhow!("malformed human safe-to-rerun line: {human_safe}"))?;
    ensure!(human_safe_value == "no");
    let json_safe_reason = report["outcome"]["safe_to_rerun"]["reason"]
        .as_str()
        .ok_or_else(|| anyhow!("missing typed safe-to-rerun reason"))?;
    ensure!(
        normalize_state_identity(human_safe_reason, &human.state_dir)
            == normalize_state_identity(json_safe_reason, &json.state_dir),
        "human={human_safe_reason:?} json={json_safe_reason:?}"
    );
    let next_reason = report["outcome"]["next_action"]["reason"]
        .as_str()
        .ok_or_else(|| anyhow!("missing typed next-action reason"))?;
    let human_next = normalize_state_identity(human_next, &human.state_dir);
    let json_next = normalize_state_identity(next_reason, &json.state_dir);
    ensure!(
        human_next == json_next,
        "human={human_next:?} json={json_next:?}"
    );
    let json_evidence = report["outcome"]["evidence"]
        .as_array()
        .ok_or_else(|| anyhow!("missing typed evidence"))?;
    let json_evidence = json_evidence
        .iter()
        .map(|evidence| {
            evidence
                .as_str()
                .ok_or_else(|| anyhow!("non-string typed evidence"))
        })
        .collect::<Result<Vec<_>>>()?;
    let human_evidence = human_evidence.split(", ").collect::<Vec<_>>();
    let expected_artifacts = ["state.json", "events.jsonl", "receipt.json"];
    let expected_human_evidence = expected_artifacts
        .iter()
        .map(|artifact| human.state_dir.join(artifact).display().to_string())
        .collect::<Vec<_>>();
    let expected_json_evidence = expected_artifacts
        .iter()
        .map(|artifact| json.state_dir.join(artifact).display().to_string())
        .collect::<Vec<_>>();
    ensure!(human_evidence == expected_human_evidence);
    ensure!(json_evidence == expected_json_evidence);
    let normalized_human_evidence = human_evidence
        .iter()
        .map(|evidence| normalize_state_identity(evidence, &human.state_dir))
        .collect::<Vec<_>>();
    let normalized_json_evidence = json_evidence
        .iter()
        .map(|evidence| normalize_state_identity(evidence, &json.state_dir))
        .collect::<Vec<_>>();
    ensure!(normalized_human_evidence == normalized_json_evidence);

    assert_registry_completion_artifacts(
        &human.state_dir,
        "crates-io",
        "partial_failure",
        "pending",
    );
    assert_registry_completion_artifacts(
        &json.state_dir,
        "crates-io",
        "partial_failure",
        "pending",
    );
    assert_secret_absent_from_output_and_state(&human.output, &human.state_dir);
    assert_secret_absent_from_output_and_state(&json.output, &json.state_dir);

    Ok(())
}

#[test]
fn ambiguous_still_unknown_publish_human_and_json_have_safe_error_parity() -> Result<()> {
    const SECRET: &str = "AMBIGUOUS_OUTCOME_SECRET";

    let human = run_ambiguous_still_unknown_publish(None)?;
    let json = run_ambiguous_still_unknown_publish(Some("json"))?;
    ensure!(human.output.status.code() == Some(1));
    ensure!(json.output.status.code() == Some(1));
    ensure!(
        human.output.stdout.is_empty(),
        "human error stays on stderr"
    );
    ensure!(json.output.stdout.is_empty(), "JSON error stays on stderr");

    let human_stderr = std::str::from_utf8(&human.output.stderr).context("human stderr")?;
    let human_result = human_outcome_value(human_stderr, "Result:")?;
    let human_safe = human_outcome_value(human_stderr, "Safe to rerun:")?;
    let human_next = human_outcome_value(human_stderr, "Next:")?;
    let human_evidence = human_outcome_value(human_stderr, "Evidence:")?;

    let report: serde_json::Value =
        serde_json::from_slice(&json.output.stderr).with_context(|| {
            format!(
                "StillUnknown publish JSON stderr:\n{}",
                String::from_utf8_lossy(&json.output.stderr)
            )
        })?;
    ensure!(report["schema_version"] == "shipper.publish.error.v1");
    ensure!(report["command"] == "publish");
    ensure!(report["status"] == "failed");
    ensure!(report["category"] == "ambiguous");
    ensure!(report["safe_to_rerun"]["value"] == false);
    ensure!(report["next_action"]["kind"] == "reconcile");
    let next_action = report["next_action"]
        .as_object()
        .ok_or_else(|| anyhow!("missing JSON next action"))?;
    ensure!(
        !next_action.contains_key("command"),
        "commandless action must omit command: {next_action:?}"
    );
    ensure!(report["next_action"]["requires_confirmation"] == false);

    let summary = report["summary"]
        .as_str()
        .ok_or_else(|| anyhow!("missing JSON error summary"))?;
    ensure!(human_result.starts_with("failed "));
    ensure!(human_result.ends_with(summary));
    let safe_reason = report["safe_to_rerun"]["reason"]
        .as_str()
        .ok_or_else(|| anyhow!("missing JSON safe-to-rerun reason"))?;
    ensure!(human_safe.starts_with("no "));
    ensure!(human_safe.ends_with(safe_reason));
    ensure!(
        human_next
            == report["next_action"]["reason"]
                .as_str()
                .ok_or_else(|| anyhow!("missing JSON next-action reason"))?
    );

    let json_evidence = report["evidence"]
        .as_array()
        .ok_or_else(|| anyhow!("missing JSON evidence"))?
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| anyhow!("non-string JSON evidence"))
        })
        .collect::<Result<Vec<_>>>()?;
    let human_evidence = human_evidence.split(", ").collect::<Vec<_>>();
    let artifacts = ["state.json", "events.jsonl", "reconciliation.json"];
    let expected_human_evidence = artifacts
        .iter()
        .map(|artifact| human.state_dir.join(artifact).display().to_string())
        .collect::<Vec<_>>();
    let expected_json_evidence = artifacts
        .iter()
        .map(|artifact| json.state_dir.join(artifact).display().to_string())
        .collect::<Vec<_>>();
    ensure!(human_evidence == expected_human_evidence);
    ensure!(json_evidence == expected_json_evidence);
    let normalized_human = human_evidence
        .iter()
        .map(|path| normalize_state_identity(path, &human.state_dir))
        .collect::<Vec<_>>();
    let normalized_json = json_evidence
        .iter()
        .map(|path| normalize_state_identity(path, &json.state_dir))
        .collect::<Vec<_>>();
    ensure!(normalized_human == normalized_json);

    for run in [&human, &json] {
        let state_path = run.state_dir.join("state.json");
        let events_path = run.state_dir.join("events.jsonl");
        let reconciliation_path = run.state_dir.join("reconciliation.json");
        ensure!(state_path.exists());
        ensure!(events_path.exists());
        ensure!(reconciliation_path.exists());
        ensure!(!run.state_dir.join("receipt.json").exists());

        let state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&state_path).context("read state")?)
                .context("parse state")?;
        ensure!(state["packages"]["demo@0.1.0"]["state"]["state"] == "ambiguous");
        ensure!(state["packages"]["demo@0.1.0"]["attempts"] == 1);

        let events = fs::read_to_string(&events_path).context("read events")?;
        let still_unknown_count = events.lines().try_fold(0, |count, line| {
            let event: serde_json::Value = serde_json::from_str(line).context("parse event")?;
            Ok::<_, anyhow::Error>(
                count
                    + usize::from(
                        event["event_type"]["type"] == "publish_reconciled"
                            && event["event_type"]["outcome"]["outcome"] == "still_unknown",
                    ),
            )
        })?;
        ensure!(still_unknown_count == 1);

        let reconciliation: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&reconciliation_path).context("read reconciliation")?,
        )
        .context("parse reconciliation")?;
        ensure!(reconciliation["records"].as_array().map(Vec::len) == Some(1));
        ensure!(reconciliation["records"][0]["name"] == "demo");
        ensure!(reconciliation["records"][0]["version"] == "0.1.0");
        ensure!(reconciliation["records"][0]["outcome"]["outcome"] == "still_unknown");

        ensure!(read_publish_log(&run.publish_log).len() == 1);
        assert_sentinel_absent_from_output_and_state(&run.output, &run.state_dir, SECRET);
        let reconciliation_bytes = fs::read(&reconciliation_path).context("scan reconciliation")?;
        ensure!(!String::from_utf8_lossy(&reconciliation_bytes).contains(SECRET));
    }

    Ok(())
}

#[test]
fn multi_registry_later_success_does_not_mask_earlier_partial_result() {
    const SECRET: &str = "MULTI_REGISTRY_OUTCOME_SECRET";

    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let alpha = spawn_bounded_registry(vec![404], 1);
    let beta = spawn_bounded_registry(vec![200], 1);
    let config = td.path().join("multi-registry.toml");
    write_file(
        &config,
        &format!(
            r#"
schema_version = "shipper.config.v1"

[[registries.registries]]
name = "alpha"
api_base = "{alpha}"
index_base = "{alpha}"

[[registries.registries]]
name = "beta"
api_base = "{beta}"
index_base = "{beta}"
"#,
            alpha = alpha.base_url,
            beta = beta.base_url,
        ),
    );

    let state_dir = td.path().join("state-multi-registry-outcome");
    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&config)
        .arg("--registries")
        .arg("alpha,beta")
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--verify-timeout")
        .arg("0ms")
        .arg("--verify-poll")
        .arg("0ms")
        .arg("--max-attempts")
        .arg("0")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .assert()
        .get_output()
        .clone();

    assert_eq!(
        output.status.code(),
        Some(2),
        "the later successful registry must not mask alpha's partial result"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches("Publishing to registry: alpha").count(),
        1,
        "alpha heading must be unique: {stdout}"
    );
    assert_eq!(
        stdout.matches("Publishing to registry: beta").count(),
        1,
        "beta heading must be unique: {stdout}"
    );
    let alpha_heading = stdout
        .find("Publishing to registry: alpha")
        .expect("alpha heading");
    let beta_heading = stdout
        .find("Publishing to registry: beta")
        .expect("beta heading");
    assert!(alpha_heading < beta_heading, "registry order: {stdout}");
    let alpha_section = &stdout[alpha_heading..beta_heading];
    let beta_section = &stdout[beta_heading..];
    assert!(
        alpha_section.contains("Result: partial failure"),
        "alpha section must report partial failure: {alpha_section}"
    );
    assert!(
        !alpha_section.contains("Result: success"),
        "alpha section must not report success: {alpha_section}"
    );
    assert!(
        beta_section.contains("Result: success"),
        "beta section must report success: {beta_section}"
    );
    assert!(
        !beta_section.contains("Result: partial failure"),
        "beta section must not report partial failure: {beta_section}"
    );

    for (registry, expected_result, expected_state) in [
        ("alpha", "partial_failure", "pending"),
        ("beta", "success", "skipped"),
    ] {
        let registry_state = state_dir.join(registry);
        assert_registry_completion_artifacts(
            &registry_state,
            registry,
            expected_result,
            expected_state,
        );
    }

    assert_sentinel_absent_from_output_and_state(&output, &state_dir, SECRET);
    alpha
        .finish(Duration::from_secs(2))
        .expect("alpha registry completion");
    beta.finish(Duration::from_secs(2))
        .expect("beta registry completion");
}

#[test]
fn bounded_registry_reports_a_missing_request_before_its_server_timeout() {
    let registry = spawn_bounded_registry(vec![200], 1);
    let error = registry
        .finish(Duration::from_millis(50))
        .expect_err("missing request must fail the completion receipt");
    let error = format!("{error:#}");
    assert!(error.contains("deadline elapsed"), "{error}");
    assert!(error.contains("expected 1 requests"), "{error}");
    assert!(error.contains("observed 0"), "{error}");
}

#[test]
fn bounded_registry_reports_an_extra_request_after_its_expected_count() {
    let registry = spawn_bounded_registry(vec![200], 1);
    send_registry_request(&registry.base_url).expect("first registry request");
    send_registry_request(&registry.base_url).expect("extra registry request");
    let error = registry
        .finish(Duration::from_secs(1))
        .expect_err("extra request must fail the completion receipt");
    let error = format!("{error:#}");
    assert!(error.contains("request mismatch"), "{error}");
    assert!(error.contains("expected 1"), "{error}");
    assert!(error.contains("observed 2"), "{error}");
}

#[test]
fn registry_diagnostic_client_times_out_after_an_accepted_unanswered_request() {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let (accepted_tx, accepted_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let server_thread = thread::spawn(move || {
        let request = server
            .recv_timeout(Duration::from_secs(1))
            .expect("receive unanswered request")
            .expect("diagnostic client must connect");
        accepted_tx.send(()).expect("report accepted request");
        release_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("test must release the held request");
        drop(request);
    });

    let started = Instant::now();
    let (client_tx, client_rx) = mpsc::sync_channel(1);
    let client_thread = thread::spawn(move || {
        let _ = client_tx.send(send_registry_request(&base_url));
    });
    accepted_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("server must accept the diagnostic request");
    let client_result = client_rx.recv_timeout(Duration::from_millis(750));
    let client_elapsed = started.elapsed();
    release_tx.send(()).expect("release held request");
    server_thread.join().expect("join unanswered server");
    client_thread.join().expect("join diagnostic client");
    let error = client_result
        .expect("diagnostic client must honor its read deadline")
        .expect_err("accepted request without a response must hit the read deadline");
    let error = format!("{error:#}");
    assert!(error.contains("read mock registry response"), "{error}");
    assert!(
        client_elapsed < Duration::from_millis(750),
        "diagnostic client exceeded its bounded deadline: {:?}",
        client_elapsed
    );
}

#[test]
fn publish_usage_error_keeps_clap_exit_two_without_execution_envelope() {
    let td = tempdir().expect("tempdir");
    let output = shipper_cmd()
        .timeout(Duration::from_secs(20))
        .current_dir(td.path())
        .arg("--definitely-invalid")
        .arg("publish")
        .assert()
        .get_output()
        .clone();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unexpected argument"), "{stderr}");
    assert!(stderr.contains("Usage:"), "{stderr}");
    assert!(!stderr.contains("shipper.publish.error.v1"), "{stderr}");
    assert!(!stderr.contains("Safe to rerun"), "{stderr}");
    assert!(!td.path().join(".shipper").exists());
}

// ============================================================================
// Test 2: Multi-crate workspace respects dependency ordering
// ============================================================================

#[test]
fn multi_crate_publish_respects_dependency_order() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // 3 crates × (version-check 404 + readiness 200) = 6 requests
    let registry = spawn_registry(vec![404, 200, 404, 200, 404, 200], 6);

    let state_dir = td.path().join(".shipper");

    let output = loopback_shipper_cmd()
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

    // All 3 packages should be published
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse");
    let packages = receipt["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 3, "all 3 packages should be in receipt");

    let published_count = packages
        .iter()
        .filter(|p| p["state"]["state"].as_str() == Some("published"))
        .count();
    assert_eq!(published_count, 3, "all 3 packages should be published");

    // Verify dependency order: core appears before utils, utils before app in stdout
    let core_pos = stdout.find("core@0.1.0").expect("core in output");
    let utils_pos = stdout.find("utils@0.1.0").expect("utils in output");
    let app_pos = stdout.find("app@0.1.0").expect("app in output");
    assert!(
        core_pos < utils_pos,
        "core should be published before utils"
    );
    assert!(utils_pos < app_pos, "utils should be published before app");

    registry.join();
}

// ============================================================================
// Test 3: Publish with --policy fast + preflight doesn't create state for plan
// ============================================================================

#[test]
fn plan_does_not_write_state() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());

    loopback_shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success();

    // plan should never create .shipper directory
    assert!(
        !td.path().join(".shipper").exists(),
        "plan should not create .shipper state directory"
    );
}

// ============================================================================
// Test 4: Publish with --package limits scope
// ============================================================================

#[test]
fn publish_with_package_flag_limits_scope() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // Only 1 crate: version-check 404 + readiness 200 = 2 requests
    let registry = spawn_registry(vec![404, 200], 2);

    let state_dir = td.path().join(".shipper");

    loopback_shipper_cmd()
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
        .arg("--package")
        .arg("core")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success();

    // Receipt should only have core, not utils or app
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse");
    let packages = receipt["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 1, "only one package should be published");
    assert_eq!(packages[0]["name"].as_str(), Some("core"));
    assert_eq!(packages[0]["state"]["state"].as_str(), Some("published"));

    registry.join();
}

// ============================================================================
// Test 5: Publish creates events.jsonl with correct lifecycle events
// ============================================================================

#[test]
fn publish_creates_events_jsonl_with_lifecycle_events() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    let registry = spawn_registry(vec![404, 200], 2);
    let state_dir = td.path().join(".shipper");

    loopback_shipper_cmd()
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
        .success();

    let events_path = state_dir.join("events.jsonl");
    assert!(events_path.exists(), "events.jsonl should be created");

    let events_content = fs::read_to_string(&events_path).expect("read events");
    assert!(
        !events_content.is_empty(),
        "events.jsonl should not be empty"
    );

    // Verify lifecycle events are present
    assert!(
        events_content.contains(r#""type":"plan_created"#),
        "should contain plan_created event"
    );
    assert!(
        events_content.contains(r#""type":"execution_started"#),
        "should contain execution_started event"
    );
    assert!(
        events_content.contains(r#""type":"package_started"#),
        "should contain package_started event"
    );
    assert!(
        events_content.contains(r#""type":"package_published"#),
        "should contain package_published event"
    );
    assert!(
        events_content.contains(r#""type":"execution_finished"#),
        "should contain execution_finished event"
    );

    // Each line should be valid JSON
    for line in events_content.lines() {
        let _: serde_json::Value =
            serde_json::from_str(line).expect("each events.jsonl line should be valid JSON");
    }

    registry.join();
}

// ============================================================================
// Test 6: Publish with custom --state-dir writes to correct location
// ============================================================================

#[test]
fn publish_with_custom_state_dir_writes_to_correct_location() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    let registry = spawn_registry(vec![404, 200], 2);
    let custom_dir = td.path().join("my-artifacts").join("nested");

    loopback_shipper_cmd()
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
        .arg(&custom_dir)
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .assert()
        .success();

    // All state files should be in the custom directory
    assert!(custom_dir.exists(), "custom state dir should be created");
    assert!(
        custom_dir.join("state.json").exists(),
        "state.json should be in custom dir"
    );
    assert!(
        custom_dir.join("receipt.json").exists(),
        "receipt.json should be in custom dir"
    );
    assert!(
        custom_dir.join("events.jsonl").exists(),
        "events.jsonl should be in custom dir"
    );

    // Default .shipper directory should NOT be created
    assert!(
        !td.path().join(".shipper").exists(),
        "default .shipper dir should not be created when custom --state-dir is used"
    );

    // Verify receipt content is correct
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(custom_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse receipt");
    assert!(receipt.get("plan_id").is_some());

    registry.join();
}

// ============================================================================
// Test 7: Failed publish creates appropriate state for resume
// ============================================================================

#[test]
fn failed_publish_creates_state_for_resume() {
    let td = tempdir().expect("tempdir");
    create_single_crate_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // version-check 404 (not published), then registry confirms absence after
    // a permanent cargo failure.
    let registry = spawn_registry(vec![404, 404], 2);
    let state_dir = td.path().join(".shipper");

    loopback_shipper_cmd()
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
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
        .env(
            "SHIPPER_FAKE_PUBLISH_STDERR",
            "error: not authorized to publish this crate",
        )
        .assert()
        .failure();

    // State file should still be created even after failure
    let state_path = state_dir.join("state.json");
    assert!(
        state_path.exists(),
        "state.json should exist after failed publish (for resume)"
    );

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&state_path).expect("read state"))
            .expect("parse state");
    assert!(state.get("plan_id").is_some(), "state should have plan_id");
    assert!(
        state.get("packages").is_some(),
        "state should have packages"
    );

    // The demo package should be in a non-published state (failed or pending)
    let packages = state["packages"].as_object().expect("packages map");
    let demo = packages.get("demo@0.1.0").expect("demo in packages");
    let demo_state = demo["state"]["state"].as_str().expect("state string");
    assert_ne!(
        demo_state, "published",
        "failed package should NOT be marked published"
    );

    registry.join();
}

// ============================================================================
// Test 8: Re-running publish when all crates already published skips everything
// ============================================================================

#[test]
fn publish_when_already_published_skips_all() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // Registry returns 200 for each version check (already published).
    let registry = spawn_registry(vec![200, 200, 200], 3);
    let state_dir = td.path().join(".shipper");
    let publish_log = td.path().join("publish.log");

    let output = loopback_shipper_cmd()
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
        .arg("--format")
        .arg("json")
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    let report: serde_json::Value =
        serde_json::from_str(&stdout).expect("publish stdout should be command JSON");
    let report_packages = report["packages"].as_array().expect("report packages");

    assert_eq!(report_packages.len(), 3, "all packages should be reported");
    assert_eq!(report["published"].as_u64(), Some(0));
    assert_eq!(report["pending"].as_u64(), Some(0));
    assert_eq!(report["failed"].as_u64(), Some(0));
    assert_eq!(report["ambiguous"].as_u64(), Some(0));
    assert_eq!(report["uploaded"].as_u64(), Some(0));
    assert_eq!(report["skipped"].as_u64(), Some(3));
    for package in ["core", "utils", "app"] {
        assert_eq!(
            command_package_state(report_packages, package),
            "skipped",
            "{package} should be skipped in publish JSON"
        );
    }
    assert!(
        read_publish_log(&publish_log).is_empty(),
        "cargo publish must not run when every version already exists"
    );

    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse");
    let packages = receipt["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 3, "all packages should be in receipt");
    for package in ["core", "utils", "app"] {
        assert_eq!(
            receipt_package_state(packages, package),
            "skipped",
            "{package} should be skipped in receipt"
        );
    }

    registry.join();
}

#[test]
fn publish_mixed_existing_and_missing_publishes_missing_only() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // core exists; utils and app are missing and become visible after publish.
    let registry = spawn_registry(vec![200, 404, 200, 404, 200], 5);
    let state_dir = td.path().join(".shipper");
    let publish_log = td.path().join("publish.log");

    let output = loopback_shipper_cmd()
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
        .arg("--format")
        .arg("json")
        .arg("publish")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    let report: serde_json::Value =
        serde_json::from_str(&stdout).expect("publish stdout should be command JSON");
    let report_packages = report["packages"].as_array().expect("report packages");

    assert_eq!(command_package_state(report_packages, "core"), "skipped");
    assert_eq!(command_package_state(report_packages, "utils"), "published");
    assert_eq!(command_package_state(report_packages, "app"), "published");
    assert_eq!(report["published"].as_u64(), Some(2));
    assert_eq!(report["pending"].as_u64(), Some(0));
    assert_eq!(report["failed"].as_u64(), Some(0));
    assert_eq!(report["ambiguous"].as_u64(), Some(0));
    assert_eq!(report["uploaded"].as_u64(), Some(0));
    assert_eq!(report["skipped"].as_u64(), Some(1));

    let publish_log = read_publish_log(&publish_log);
    assert_eq!(
        publish_log.len(),
        2,
        "only missing package versions should invoke cargo publish"
    );
    assert!(
        publish_log[0].contains("-p utils"),
        "utils should publish after skipped core, log: {publish_log:?}"
    );
    assert!(
        publish_log[1].contains("-p app"),
        "app should publish after utils, log: {publish_log:?}"
    );
    assert!(
        publish_log.iter().all(|line| !line.contains("-p core")),
        "already-published core must not invoke cargo publish, log: {publish_log:?}"
    );

    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("receipt.json")).expect("read receipt"),
    )
    .expect("parse receipt");
    let packages = receipt["packages"].as_array().expect("receipt packages");
    assert_eq!(receipt_package_state(packages, "core"), "skipped");
    assert_eq!(receipt_package_state(packages, "utils"), "published");
    assert_eq!(receipt_package_state(packages, "app"), "published");

    registry.join();
}

#[test]
fn publish_mixed_existing_and_missing_failure_records_failed_package() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());

    // core exists; utils is missing, cargo publish fails, registry confirms absent.
    let registry = spawn_registry(vec![200, 404, 404], 3);
    let state_dir = td.path().join(".shipper");
    let publish_log = td.path().join("publish.log");

    loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
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
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
        .env(
            "SHIPPER_FAKE_PUBLISH_STDERR",
            "error: not authorized to publish this crate",
        )
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .assert()
        .failure();

    let publish_log = read_publish_log(&publish_log);
    assert_eq!(
        publish_log.len(),
        1,
        "failure should stop after first missing package publish attempt"
    );
    assert!(
        publish_log[0].contains("-p utils"),
        "utils should be the failed publish attempt, log: {publish_log:?}"
    );
    assert!(
        !publish_log[0].contains("-p core") && !publish_log[0].contains("-p app"),
        "already-published and downstream packages must not be published, log: {publish_log:?}"
    );

    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state_dir.join("state.json")).expect("read state"),
    )
    .expect("parse state");
    assert_eq!(
        state["packages"]["core@0.1.0"]["state"]["state"].as_str(),
        Some("skipped"),
        "already-published core should be recorded as skipped"
    );
    assert_eq!(
        state["packages"]["utils@0.1.0"]["state"]["state"].as_str(),
        Some("failed"),
        "failed missing package should be recorded as failed"
    );
    assert_eq!(
        state["packages"]["app@0.1.0"]["state"]["state"].as_str(),
        Some("pending"),
        "downstream package should remain pending after upstream failure"
    );

    registry.join();
}

#[test]
fn controlled_stop_initial_human_and_json_envelopes_have_semantic_parity() -> Result<()> {
    let human = run_initial_controlled_stop_publish(None, false)?;
    let json = run_initial_controlled_stop_publish(Some("json"), false)?;
    ensure!(human.output.status.code() == Some(1));
    ensure!(json.output.status.code() == Some(1));
    ensure!(human.output.stdout.is_empty());
    ensure!(json.output.stdout.is_empty());

    let human_stderr = String::from_utf8(human.output.stderr.clone())?;
    let human_result = human_outcome_value(&human_stderr, "Result:")?;
    let human_safe = human_outcome_value(&human_stderr, "Safe to rerun:")?;
    let human_next = human_outcome_value(&human_stderr, "Next:")?;
    let human_evidence = human_outcome_value(&human_stderr, "Evidence:")?;
    ensure!(human_result.starts_with("failed"));
    ensure!(human_safe.starts_with("no"));

    let report: serde_json::Value = serde_json::from_slice(&json.output.stderr)?;
    ensure!(report["schema_version"] == "shipper.publish.error.v1");
    ensure!(report["category"] == "recoverable_stop");
    ensure!(report["safe_to_rerun"]["value"] == false);
    ensure!(
        report["safe_to_rerun"]["reason"]
            == "do not rerun publish; retained evidence authorizes controlled resume"
    );
    ensure!(
        human_safe.ends_with(
            report["safe_to_rerun"]["reason"]
                .as_str()
                .context("safe-to-rerun reason")?
        ),
        "human={human_safe:?} JSON={:?}",
        report["safe_to_rerun"]["reason"]
    );
    ensure!(report["next_action"]["kind"] == "resume");
    ensure!(report["next_action"].get("command").is_none());
    ensure!(
        human_next
            == report["next_action"]["reason"]
                .as_str()
                .context("next reason")?
    );

    let human_evidence = human_evidence.split(", ").collect::<Vec<_>>();
    let json_evidence = report["evidence"]
        .as_array()
        .context("JSON evidence")?
        .iter()
        .map(|value| value.as_str().context("string evidence"))
        .collect::<Result<Vec<_>>>()?;
    ensure!(human_evidence.len() == 3);
    ensure!(json_evidence.len() == 3);
    let normalize = |paths: &[&str], state_dir: &Path| {
        paths
            .iter()
            .map(|path| normalize_state_identity(path, state_dir))
            .collect::<Vec<_>>()
    };
    ensure!(
        normalize(&human_evidence, &human.state_dir) == normalize(&json_evidence, &json.state_dir)
    );
    for run in [&human, &json] {
        ensure!(run.state_dir.join("state.json").exists());
        ensure!(run.state_dir.join("events.jsonl").exists());
        ensure!(run.state_dir.join("reconciliation.json").exists());
        ensure!(!run.state_dir.join("receipt.json").exists());
        assert_sentinel_absent_from_output_and_state(
            &run.output,
            &run.state_dir,
            "CONTROLLED_STOP_SECRET_SENTINEL",
        );
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn controlled_stop_without_reconciliation_denies_resume_in_human_and_json() -> Result<()> {
    let human = run_initial_controlled_stop_publish(None, true)?;
    let json = run_initial_controlled_stop_publish(Some("json"), true)?;
    ensure!(human.output.status.code() == Some(1));
    ensure!(json.output.status.code() == Some(1));
    let human_stderr = String::from_utf8(human.output.stderr.clone())?;
    ensure!(
        human_stderr.contains(
            "retained recovery evidence is missing or inconsistent; recovery is not authorized"
        ),
        "{human_stderr}"
    );
    let human_safe = human_outcome_value(&human_stderr, "Safe to rerun:")?;
    let human_next = human_outcome_value(&human_stderr, "Next:")?;
    ensure!(human_safe.starts_with("no"));
    ensure!(human_safe.contains("recovery safety is not proven"));
    ensure!(human_next.contains("do not resume"));
    ensure!(!human_stderr.contains("authorizes a controlled resume"));

    let report: serde_json::Value = serde_json::from_slice(&json.output.stderr)?;
    ensure!(report["category"] == "recoverable_stop");
    ensure!(report["safe_to_rerun"]["value"] == false);
    ensure!(
        report["safe_to_rerun"]["reason"]
            == "retained recovery evidence is missing or inconsistent, so recovery safety is not proven"
    );
    ensure!(report["next_action"]["kind"] == "inspect_events");
    ensure!(
        report["next_action"]["reason"]
            == "inspect retained events, state, and reconciliation evidence; do not resume while evidence is missing or inconsistent"
    );
    ensure!(report["evidence"].as_array().map(Vec::len) == Some(2));
    ensure!(
        !String::from_utf8_lossy(&json.output.stderr).contains("authorizes a controlled resume")
    );
    for run in [&human, &json] {
        ensure!(run.state_dir.join("state.json").exists());
        ensure!(run.state_dir.join("events.jsonl").exists());
        ensure!(run.state_dir.join("reconciliation.json").is_dir());
        ensure!(!run.state_dir.join("receipt.json").exists());
        assert_sentinel_absent_from_output_and_state(
            &run.output,
            &run.state_dir,
            "CONTROLLED_STOP_SECRET_SENTINEL",
        );
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn parallel_incomplete_reconciliation_never_authorizes_resume() -> Result<()> {
    let human = run_parallel_inconsistent_controlled_stop(None)?;
    let json = run_parallel_inconsistent_controlled_stop(Some("json"))?;
    ensure!(human.output.status.code() == Some(1));
    ensure!(json.output.status.code() == Some(1));
    let human_stderr = String::from_utf8(human.output.stderr.clone())?;
    ensure!(
        human_stderr.contains("recovery is not authorized"),
        "{human_stderr}"
    );
    ensure!(!human_stderr.contains("authorizes a controlled resume"));
    let report: serde_json::Value = serde_json::from_slice(&json.output.stderr)?;
    ensure!(report["safe_to_rerun"]["value"] == false);
    ensure!(report["next_action"]["kind"] == "inspect_events");
    ensure!(read_publish_log(&human.publish_log).len() == 2);

    let state_before = fs::read(human.state_dir.join("state.json"))?;
    let events_before = fs::read(human.state_dir.join("events.jsonl"))?;
    let cargo_before = fs::read(&human.publish_log)?;
    let status = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .current_dir(human.workspace.path())
        .arg("--manifest-path")
        .arg(human.workspace.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&human.api_base)
        .arg("--state-dir")
        .arg(&human.state_dir)
        .arg("--format")
        .arg("json")
        .arg("status")
        .arg("--durable")
        .output()?;
    if status.status.success() {
        let status_json: serde_json::Value = serde_json::from_slice(&status.stdout)?;
        ensure!(status_json["outcome"]["next_action"]["kind"] != "resume");
        ensure!(status_json["outcome"]["safe_to_resume"]["value"] != true);
    } else {
        ensure!(status.stdout.is_empty());
    }

    let resume = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .current_dir(human.workspace.path())
        .arg("--manifest-path")
        .arg(human.workspace.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&human.api_base)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--state-dir")
        .arg(&human.state_dir)
        .arg("resume")
        .env("PATH", &human.new_path)
        .env("REAL_CARGO", &human.real_cargo)
        .env("SHIPPER_CARGO_BIN", &human.fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_LOG", &human.publish_log)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .output()?;
    ensure!(!resume.status.success());
    ensure!(fs::read(human.state_dir.join("state.json"))? == state_before);
    ensure!(fs::read(human.state_dir.join("events.jsonl"))? == events_before);
    ensure!(fs::read(&human.publish_log)? == cargo_before);
    Ok(())
}

#[test]
fn conclusive_not_published_stop_status_and_resume_preserve_completed_packages() -> Result<()> {
    const SECRET: &str = "CONTROLLED_STOP_SECRET_SENTINEL";

    let td = tempdir().context("create controlled-stop workspace")?;
    create_workspace(td.path());
    let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(td.path());
    let state_dir = td.path().join("controlled-stop-state");
    let publish_log = td.path().join("controlled-stop-publish.log");

    // Core and utils already exist. App is absent before and after its ambiguous
    // Cargo failure, so registry truth conclusively permits a controlled stop.
    // The resumed precheck must also confirm absence before the successful
    // retry's post-publish visibility check returns 200.
    let first_registry = spawn_bounded_registry(vec![200, 200, 404, 404, 404, 200], 6);
    let first_registry_base = first_registry.base_url.clone();
    let first = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .current_dir(td.path())
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&first_registry_base)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--verify-timeout")
        .arg("0ms")
        .arg("--verify-poll")
        .arg("0ms")
        .arg("--readiness-timeout")
        .arg("0ms")
        .arg("--readiness-poll")
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
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
        .env("SHIPPER_FAKE_PUBLISH_STDERR", "ambiguous transport close")
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .output()
        .context("run controlled-stop publish")?;
    ensure!(
        first.status.code() == Some(1),
        "publish status={:?}",
        first.status.code()
    );
    ensure!(state_dir.join("state.json").exists());
    ensure!(state_dir.join("events.jsonl").exists());
    ensure!(state_dir.join("reconciliation.json").exists());
    ensure!(!state_dir.join("receipt.json").exists());
    assert_sentinel_absent_from_output_and_state(&first, &state_dir, SECRET);

    let initial_state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state_dir.join("state.json"))?)?;
    ensure!(initial_state["packages"]["app@0.1.0"]["state"]["state"] == "failed");
    ensure!(initial_state["packages"]["app@0.1.0"]["state"]["class"] == "retryable");
    ensure!(initial_state["packages"]["app@0.1.0"]["attempts"] == 1);
    let event_values = fs::read_to_string(state_dir.join("events.jsonl"))?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let terminal = event_values
        .last()
        .context("terminal controlled-stop event")?;
    ensure!(terminal["event_type"]["type"] == "execution_stopped");
    ensure!(terminal["event_type"]["reason"] == "not_published_retry_budget_exhausted");
    ensure!(terminal["package"] == "app@0.1.0");
    let reconciliation_path = state_dir.join("reconciliation.json");
    let reconciliation_bytes = fs::read(&reconciliation_path)?;
    let reconciliation: serde_json::Value = serde_json::from_slice(&reconciliation_bytes)?;
    ensure!(reconciliation["plan_id"] == initial_state["plan_id"]);
    ensure!(reconciliation["registry"] == initial_state["registry"]);
    let latest = reconciliation["records"]
        .as_array()
        .context("reconciliation records")?
        .last()
        .context("latest reconciliation record")?;
    ensure!(latest["package"] == "app@0.1.0");
    ensure!(latest["outcome"]["outcome"] == "not_published");
    ensure!(latest["operator_action"] == "retry_allowed");

    let first_calls = read_publish_log(&publish_log);
    ensure!(
        first_calls.len() == 1,
        "initial Cargo calls={first_calls:?}"
    );
    ensure!(
        first_calls[0].contains("-p app"),
        "initial Cargo calls={first_calls:?}"
    );

    let invoke_status = |json: bool| -> Result<std::process::Output> {
        let mut command = loopback_shipper_cmd();
        command
            .timeout(Duration::from_secs(20))
            .current_dir(td.path())
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&first_registry_base)
            .arg("--state-dir")
            .arg(&state_dir);
        if json {
            command.arg("--format").arg("json");
        }
        Ok(command
            .arg("status")
            .arg("--durable")
            .env("CARGO_REGISTRY_TOKEN", SECRET)
            .output()?)
    };

    let mut corrupt_reconciliation = reconciliation.clone();
    let mut corrupt_duplicate = latest.clone();
    corrupt_duplicate["name"] = serde_json::Value::String("forged-package-name".to_string());
    corrupt_reconciliation["records"]
        .as_array_mut()
        .context("mutable reconciliation records")?
        .push(corrupt_duplicate);
    fs::write(
        &reconciliation_path,
        serde_json::to_vec_pretty(&corrupt_reconciliation)?,
    )?;
    let corrupt_state_before = fs::read(state_dir.join("state.json"))?;
    let corrupt_events_before = fs::read(state_dir.join("events.jsonl"))?;
    let corrupt_cargo_before = fs::read(&publish_log)?;
    for corrupt_status in [invoke_status(false)?, invoke_status(true)?] {
        ensure!(corrupt_status.status.success());
        let rendered = if corrupt_status.stdout.starts_with(b"{") {
            let value: serde_json::Value = serde_json::from_slice(&corrupt_status.stdout)?;
            ensure!(value["outcome"]["safe_to_resume"]["value"] == false);
            ensure!(value["outcome"]["next_action"]["kind"] != "resume");
            value.to_string()
        } else {
            let text = String::from_utf8(corrupt_status.stdout)?;
            ensure!(text.contains("Safe to resume: no"), "{text}");
            ensure!(!text.contains("Next: resume is safe"), "{text}");
            text
        };
        ensure!(!rendered.contains(SECRET));
    }
    let corrupt_resume = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .current_dir(td.path())
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&first_registry_base)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .output()?;
    ensure!(!corrupt_resume.status.success());
    ensure!(
        String::from_utf8_lossy(&corrupt_resume.stderr)
            .contains("reconciliation package identity disagrees")
    );
    ensure!(fs::read(state_dir.join("state.json"))? == corrupt_state_before);
    ensure!(fs::read(state_dir.join("events.jsonl"))? == corrupt_events_before);
    ensure!(fs::read(&publish_log)? == corrupt_cargo_before);
    fs::write(&reconciliation_path, &reconciliation_bytes)?;

    let human = invoke_status(false)?;
    let json = invoke_status(true)?;
    ensure!(
        human.status.success(),
        "human status={:?}",
        human.status.code()
    );
    ensure!(
        json.status.success(),
        "JSON status={:?}",
        json.status.code()
    );
    let human_stdout = String::from_utf8(human.stdout.clone())?;
    ensure!(
        human_stdout.contains("Durable result: interrupted"),
        "{human_stdout}"
    );
    ensure!(
        human_stdout.contains("Safe to resume: yes"),
        "{human_stdout}"
    );
    ensure!(
        human_stdout.contains("Next: resume is safe"),
        "{human_stdout}"
    );
    let status_json: serde_json::Value = serde_json::from_slice(&json.stdout)?;
    ensure!(status_json["outcome"]["status"] == "interrupted");
    ensure!(status_json["outcome"]["safe_to_resume"]["value"] == true);
    ensure!(status_json["outcome"]["next_action"]["kind"] == "resume");
    for output in [&human, &json] {
        assert_sentinel_absent_from_output_and_state(output, &state_dir, SECRET);
    }

    // The new run segment is granted a second total attempt. Its precheck
    // confirms app is still absent, then the post-publish check sees success.
    // Only app reaches Cargo; core/utils remain retained from the first segment.
    let retained_events = fs::read_to_string(state_dir.join("events.jsonl"))?;
    let resumed = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .current_dir(td.path())
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&first_registry_base)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--verify-timeout")
        .arg("0ms")
        .arg("--verify-poll")
        .arg("0ms")
        .arg("--readiness-timeout")
        .arg("0ms")
        .arg("--readiness-poll")
        .arg("0ms")
        .arg("--max-attempts")
        .arg("2")
        .arg("--base-delay")
        .arg("0ms")
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("resume")
        .env("PATH", &new_path)
        .env("REAL_CARGO", &real_cargo)
        .env("SHIPPER_CARGO_BIN", &fake_cargo)
        .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
        .env("SHIPPER_FAKE_PUBLISH_STDERR", "")
        .env("SHIPPER_FAKE_PUBLISH_LOG", &publish_log)
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .output()
        .context("resume controlled stop")?;
    ensure!(
        resumed.status.success(),
        "resume status={:?}; stderr={}",
        resumed.status.code(),
        String::from_utf8_lossy(&resumed.stderr)
    );
    first_registry.finish(Duration::from_secs(2))?;
    assert_sentinel_absent_from_output_and_state(&resumed, &state_dir, SECRET);

    let all_calls = read_publish_log(&publish_log);
    ensure!(all_calls.len() == 2, "all Cargo calls={all_calls:?}");
    ensure!(
        all_calls.iter().all(|call| call.contains("-p app")),
        "all Cargo calls={all_calls:?}"
    );
    ensure!(
        all_calls
            .iter()
            .all(|call| !call.contains("-p core") && !call.contains("-p utils")),
        "all Cargo calls={all_calls:?}"
    );
    let final_state: serde_json::Value =
        serde_json::from_slice(&fs::read(state_dir.join("state.json"))?)?;
    ensure!(final_state["packages"]["app@0.1.0"]["attempts"] == 2);
    for key in ["core@0.1.0", "utils@0.1.0"] {
        ensure!(final_state["packages"][key]["attempts"] == 0);
        ensure!(final_state["packages"][key]["state"]["state"] == "skipped");
    }
    ensure!(
        fs::read_to_string(state_dir.join("events.jsonl"))?.starts_with(&retained_events),
        "resume must preserve the predecessor event history"
    );

    let receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state_dir.join("receipt.json"))?)?;
    let packages = receipt["packages"].as_array().context("receipt packages")?;
    ensure!(receipt_package_state(packages, "core") == "skipped");
    ensure!(receipt_package_state(packages, "utils") == "skipped");
    ensure!(receipt_package_state(packages, "app") == "published");
    ensure!(receipt["execution_result"] == "success");
    Ok(())
}
