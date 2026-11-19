use std::fs;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
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

fn loopback_shipper_cmd() -> Command {
    let mut command = shipper_cmd();
    command.arg("--allow-loopback");
    command
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

struct BoundedStatusRegistry {
    base_url: String,
    server: Arc<Server>,
    handle: thread::JoinHandle<usize>,
    completed: mpsc::Receiver<usize>,
    expected_requests: usize,
}

impl BoundedStatusRegistry {
    fn finish(self, timeout: Duration) -> Result<()> {
        let observed = match self.completed.recv_timeout(timeout) {
            Ok(observed) => observed,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.server.unblock();
                let observed = self
                    .handle
                    .join()
                    .map_err(|_| anyhow!("status registry thread panicked after deadline"))?;
                bail!(
                    "status registry deadline elapsed: expected {} requests, observed {observed}",
                    self.expected_requests
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("status registry completion channel disconnected");
            }
        };
        let joined = self
            .handle
            .join()
            .map_err(|_| anyhow!("status registry thread panicked"))?;
        if joined != observed {
            bail!(
                "status registry completion mismatch: channel reported {observed}, thread returned {joined}"
            );
        }
        if observed != self.expected_requests {
            bail!(
                "status registry request mismatch: expected {}, observed {observed}",
                self.expected_requests
            );
        }
        Ok(())
    }
}

fn spawn_bounded_status_registry(
    statuses: Vec<u16>,
    expected_requests: usize,
) -> Result<BoundedStatusRegistry> {
    let server =
        Arc::new(Server::http("127.0.0.1:0").map_err(|_| anyhow!("bind bounded status registry"))?);
    let base_url = format!("http://{}", server.server_addr());
    let worker_server = Arc::clone(&server);
    let (completed_tx, completed) = mpsc::sync_channel(1);
    let handle = thread::spawn(move || {
        let mut observed = 0;
        for idx in 0..expected_requests {
            let request = match worker_server.recv_timeout(Duration::from_secs(20)) {
                Ok(Some(request)) => request,
                _ => break,
            };
            observed += 1;
            let status = statuses
                .get(idx)
                .copied()
                .or_else(|| statuses.last().copied())
                .unwrap_or(404);
            let response = Response::from_string("{}").with_status_code(StatusCode(status));
            if request.respond(response).is_err() {
                break;
            }
        }
        if let Ok(Some(request)) = worker_server.recv_timeout(Duration::from_millis(250)) {
            observed += 1;
            let response = Response::from_string("{}").with_status_code(StatusCode(404));
            let _ = request.respond(response);
        }
        let _ = completed_tx.send(observed);
        observed
    });
    Ok(BoundedStatusRegistry {
        base_url,
        server,
        handle,
        completed,
        expected_requests,
    })
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

fn create_registry_restricted_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["public-crate", "private-crate"]
resolver = "2"
"#,
    );
    write_file(
        &root.join("public-crate/Cargo.toml"),
        r#"
[package]
name = "public-crate"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(
        &root.join("public-crate/src/lib.rs"),
        "pub fn public_crate() {}\n",
    );
    write_file(
        &root.join("private-crate/Cargo.toml"),
        r#"
[package]
name = "private-crate"
version = "0.1.0"
edition = "2021"
publish = ["private-reg"]
"#,
    );
    write_file(
        &root.join("private-crate/src/lib.rs"),
        "pub fn private_crate() {}\n",
    );
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

fn run_bounded_status(
    root: &Path,
    state_dir: &Path,
    statuses: Vec<u16>,
    format: &str,
    secret: &str,
) -> Result<std::process::Output> {
    let expected_requests = statuses.len();
    let registry = spawn_bounded_status_registry(statuses, expected_requests)?;
    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .env("CARGO_REGISTRY_TOKEN", secret)
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--format")
        .arg(format)
        .arg("status");
    let output = command.output();
    let registry_result = registry.finish(Duration::from_secs(2));
    let output = output.context("run bounded status process")?;
    registry_result?;
    Ok(output)
}

// ── status on a simple workspace ─────────────────────────────────────

fn run_bounded_durable_status(
    root: &Path,
    registry: BoundedStatusRegistry,
    format: Option<&str>,
    secret: &str,
) -> Result<std::process::Output> {
    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .env("CARGO_REGISTRY_TOKEN", secret)
        .current_dir(root)
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(".operator-state");
    if let Some(format) = format {
        command.arg("--format").arg(format);
    }
    command.arg("status").arg("--durable");
    let output = command.output().context("run bounded durable status")?;
    registry.finish(Duration::from_secs(2))?;
    Ok(output)
}

#[test]
fn durable_status_no_evidence_has_human_json_parity_without_registry_access() -> Result<()> {
    const SECRET: &str = "DURABLE_STATUS_SECRET_SENTINEL";
    let td = tempdir()?;
    create_simple_workspace(td.path());
    let state_dir = td.path().join(".operator-state");

    let human_registry = spawn_bounded_status_registry(Vec::new(), 0)?;
    let human = run_bounded_durable_status(td.path(), human_registry, None, SECRET)?;
    anyhow::ensure!(
        human.status.success(),
        "human stderr: {}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human_stdout = String::from_utf8(human.stdout).context("human stdout utf8")?;
    let human_stderr = String::from_utf8(human.stderr).context("human stderr utf8")?;
    anyhow::ensure!(human_stdout.contains("Durable result: no durable evidence"));
    anyhow::ensure!(human_stdout.contains("Publication performed: no"));
    anyhow::ensure!(human_stdout.contains("Safe to resume: unknown"));
    anyhow::ensure!(human_stdout.contains("Evidence: none"));
    anyhow::ensure!(!human_stdout.contains("shipper --state-dir"));

    let json_registry = spawn_bounded_status_registry(Vec::new(), 0)?;
    let json = run_bounded_durable_status(td.path(), json_registry, Some("json"), SECRET)?;
    anyhow::ensure!(
        json.status.success(),
        "json stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout)?;
    anyhow::ensure!(envelope["schema_version"] == "shipper.status.durable.v1");
    anyhow::ensure!(envelope["state_dir"] == ".operator-state");
    anyhow::ensure!(envelope["outcome"]["status"] == "no_evidence");
    anyhow::ensure!(envelope["outcome"]["publication_performed"] == false);
    anyhow::ensure!(envelope["outcome"]["safe_to_resume"]["value"].is_null());
    anyhow::ensure!(envelope["outcome"]["next_action"]["kind"] == "status");
    anyhow::ensure!(envelope["outcome"]["next_action"].get("command").is_none());
    anyhow::ensure!(
        human_stdout.contains("no durable run exists to interpret")
            && envelope["outcome"]["next_action"]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("no durable run exists to interpret"))
    );

    let json_stderr = String::from_utf8(json.stderr).context("json stderr utf8")?;
    for rendered in [
        &human_stdout,
        &human_stderr,
        &json_stderr,
        &envelope.to_string(),
    ] {
        anyhow::ensure!(
            !rendered.contains(SECRET),
            "secret leaked from durable status"
        );
    }
    anyhow::ensure!(
        !state_dir.exists(),
        "read-only status created durable artifacts"
    );
    Ok(())
}

#[test]
fn durable_status_conflicts_with_watch_before_evidence_or_network_access() -> Result<()> {
    let td = tempdir()?;
    create_simple_workspace(td.path());
    let output = loopback_shipper_cmd()
        .timeout(Duration::from_secs(20))
        .current_dir(td.path())
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:9")
        .arg("status")
        .arg("--watch")
        .arg("--durable")
        .output()?;
    anyhow::ensure!(output.status.code() == Some(2), "expected Clap exit 2");
    anyhow::ensure!(output.stdout.is_empty(), "parser failure wrote stdout");
    let stderr = String::from_utf8(output.stderr)?;
    anyhow::ensure!(stderr.contains("cannot be used with"));
    anyhow::ensure!(!td.path().join(".shipper").exists());
    Ok(())
}

#[test]
fn durable_status_rejects_multi_registry_selectors_before_evidence_reads() -> Result<()> {
    let td = tempdir()?;
    create_simple_workspace(td.path());
    let state_dir = td.path().join("sentinel-state");
    write_file(&state_dir.join("events.jsonl"), "not-json\n");

    for selector in [["--registries", "reg-a,reg-b"], ["--all-registries", ""]] {
        let mut command = loopback_shipper_cmd();
        command
            .timeout(Duration::from_secs(20))
            .current_dir(td.path())
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg("http://127.0.0.1:9")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg(selector[0]);
        if !selector[1].is_empty() {
            command.arg(selector[1]);
        }
        let output = command.arg("status").arg("--durable").output()?;
        anyhow::ensure!(
            output.status.code() == Some(2),
            "{}: expected Clap exit 2, got {:?}; stdout={:?}; stderr={:?}",
            selector[0],
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        anyhow::ensure!(output.stdout.is_empty(), "parser failure wrote stdout");
        let stderr = String::from_utf8(output.stderr)?;
        anyhow::ensure!(
            stderr.contains("cannot be used with"),
            "{}: parser did not report the selector conflict; stderr={stderr:?}",
            selector[0]
        );
        anyhow::ensure!(
            fs::read_to_string(state_dir.join("events.jsonl"))? == "not-json\n",
            "parser failure touched durable evidence"
        );
    }
    Ok(())
}

#[test]
fn status_rejects_loopback_without_explicit_opt_in() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("http://127.0.0.1:9")
        .arg("status")
        .assert()
        .failure()
        .stderr(contains("loopback"));
}

#[test]
fn status_simple_workspace_shows_local_versions() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    // Registry returns 404 → version not found → "missing"
    let registry = spawn_registry(vec![404], 1);

    loopback_shipper_cmd()
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

    loopback_shipper_cmd()
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

    loopback_shipper_cmd()
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

    loopback_shipper_cmd()
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

    loopback_shipper_cmd()
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

    let output = loopback_shipper_cmd()
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

    let output = loopback_shipper_cmd()
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

    loopback_shipper_cmd()
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
fn status_json_format_produces_registry_report() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    let registry = spawn_registry(vec![200], 1);

    let output = loopback_shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--format")
        .arg("json")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(
        json.pointer("/schema_version")
            .and_then(serde_json::Value::as_str),
        Some("shipper.status.v1")
    );
    assert!(json.get("plan_id").is_some());
    assert_eq!(
        json.pointer("/registries/0/name")
            .and_then(serde_json::Value::as_str),
        Some("crates-io")
    );
    assert_eq!(
        json.pointer("/registries/0/packages/0/name")
            .and_then(serde_json::Value::as_str),
        Some("alpha")
    );
    assert_eq!(
        json.pointer("/registries/0/packages/0/status")
            .and_then(serde_json::Value::as_str),
        Some("published")
    );
    assert_eq!(
        json.pointer("/registries/0/packages/0/exists")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    registry.join();
}

#[test]
fn status_plans_packages_for_each_effective_registry() -> Result<()> {
    const SECRET: &str = "STATUS_REGISTRY_PLAN_SECRET";
    let temp = tempdir().context("create restricted status workspace")?;
    create_registry_restricted_workspace(temp.path());

    let private = spawn_bounded_status_registry(vec![200, 404], 2)?;
    let private_config = temp.path().join("private-status.toml");
    write_file(
        &private_config,
        &format!(
            r#"
schema_version = "shipper.config.v1"

[registry]
name = "private-reg"
api_base = "{base}"
index_base = "{base}"
"#,
            base = private.base_url,
        ),
    );
    let private_state = temp.path().join("private-state");
    let mut private_command = loopback_shipper_cmd();
    private_command
        .timeout(Duration::from_secs(20))
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&private_config)
        .arg("--state-dir")
        .arg(&private_state)
        .arg("--format")
        .arg("json")
        .arg("status");
    let private_output = private_command.output().context("run private-reg status")?;
    private.finish(Duration::from_secs(2))?;
    anyhow::ensure!(private_output.status.code() == Some(0), "private-reg exit");
    let private_stdout = String::from_utf8(private_output.stdout).context("private JSON UTF-8")?;
    let private_stderr =
        String::from_utf8(private_output.stderr).context("private stderr UTF-8")?;
    anyhow::ensure!(!private_stdout.contains(SECRET), "private stdout secret");
    anyhow::ensure!(!private_stderr.contains(SECRET), "private stderr secret");
    let private_json: serde_json::Value =
        serde_json::from_str(&private_stdout).context("parse private status JSON")?;
    let private_packages = private_json
        .pointer("/registries/0/packages")
        .and_then(serde_json::Value::as_array)
        .context("private registry packages")?;
    anyhow::ensure!(
        private_packages.iter().any(|package| {
            package.get("name").and_then(serde_json::Value::as_str) == Some("private-crate")
        }),
        "private-only package must be queried for configured registry"
    );
    anyhow::ensure!(
        private_packages.len() == 2,
        "private registry package count"
    );
    anyhow::ensure!(
        private_json
            .pointer("/outcome/status")
            .and_then(serde_json::Value::as_str)
            == Some("partially_published"),
        "private-only missing version must prevent all-published outcome"
    );
    anyhow::ensure!(
        private_json.get("plan_id") == private_json.pointer("/registries/0/plan_id"),
        "single-registry plan identity"
    );
    anyhow::ensure!(!private_state.exists(), "private status state side effect");

    let crates_io = spawn_bounded_status_registry(vec![200], 1)?;
    let crates_state = temp.path().join("crates-state");
    let mut crates_command = loopback_shipper_cmd();
    crates_command
        .timeout(Duration::from_secs(20))
        .env("CARGO_REGISTRY_TOKEN", SECRET)
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&crates_io.base_url)
        .arg("--state-dir")
        .arg(&crates_state)
        .arg("--format")
        .arg("json")
        .arg("status");
    let crates_output = crates_command.output().context("run crates-io status")?;
    crates_io.finish(Duration::from_secs(2))?;
    anyhow::ensure!(crates_output.status.code() == Some(0), "crates-io exit");
    let crates_stdout = String::from_utf8(crates_output.stdout).context("crates JSON UTF-8")?;
    let crates_stderr = String::from_utf8(crates_output.stderr).context("crates stderr UTF-8")?;
    anyhow::ensure!(!crates_stdout.contains(SECRET), "crates stdout secret");
    anyhow::ensure!(!crates_stderr.contains(SECRET), "crates stderr secret");
    let crates_json: serde_json::Value =
        serde_json::from_str(&crates_stdout).context("parse crates status JSON")?;
    let crates_packages = crates_json
        .pointer("/registries/0/packages")
        .and_then(serde_json::Value::as_array)
        .context("crates.io registry packages")?;
    anyhow::ensure!(
        crates_packages.len() == 1,
        "crates.io must omit private-only package"
    );
    anyhow::ensure!(
        crates_packages
            .first()
            .and_then(|package| package.get("name"))
            == Some(&serde_json::Value::String("public-crate".to_string())),
        "crates.io allowed package"
    );
    anyhow::ensure!(
        crates_json
            .pointer("/outcome/status")
            .and_then(serde_json::Value::as_str)
            == Some("all_published"),
        "allowed crates.io opposite"
    );
    anyhow::ensure!(!crates_state.exists(), "crates status state side effect");

    Ok(())
}

#[test]
fn status_multi_registry_reports_registry_specific_plan_identity() -> Result<()> {
    const EXPECTED_REASON: &str = "every registry-eligible selected package version is visible in its effective target registry or registries";
    let temp = tempdir().context("create multi-registry status workspace")?;
    create_registry_restricted_workspace(temp.path());
    let crates_io = spawn_bounded_status_registry(vec![200, 200], 2)?;
    let private = spawn_bounded_status_registry(vec![200, 200, 200, 200], 4)?;
    let config = temp.path().join("multi-status.toml");
    write_file(
        &config,
        &format!(
            r#"
schema_version = "shipper.config.v1"

[[registries.registries]]
name = "crates-io"
api_base = "{crates_io}"
index_base = "{crates_io}"

[[registries.registries]]
name = "private-reg"
api_base = "{private}"
index_base = "{private}"
"#,
            crates_io = crates_io.base_url,
            private = private.base_url,
        ),
    );
    let mut command = loopback_shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&config)
        .arg("--registries")
        .arg("crates-io,private-reg")
        .arg("--format")
        .arg("json")
        .arg("status");
    let output = command.output().context("run multi-registry status")?;
    let mut human_command = loopback_shipper_cmd();
    human_command
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(temp.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&config)
        .arg("--registries")
        .arg("crates-io,private-reg")
        .arg("status");
    let human_output = human_command
        .output()
        .context("run human multi-registry status")?;
    crates_io.finish(Duration::from_secs(2))?;
    private.finish(Duration::from_secs(2))?;
    anyhow::ensure!(output.status.code() == Some(0), "multi-registry exit");
    anyhow::ensure!(
        human_output.status.code() == Some(0),
        "human multi-registry exit"
    );
    let stdout = String::from_utf8(output.stdout).context("multi-registry JSON UTF-8")?;
    let human_stdout =
        String::from_utf8(human_output.stdout).context("multi-registry human UTF-8")?;
    let json: serde_json::Value =
        serde_json::from_str(&stdout).context("parse multi-registry status JSON")?;
    let crates_packages = json
        .pointer("/registries/0/packages")
        .and_then(serde_json::Value::as_array)
        .context("multi-registry crates.io packages")?;
    let private_packages = json
        .pointer("/registries/1/packages")
        .and_then(serde_json::Value::as_array)
        .context("multi-registry private packages")?;
    anyhow::ensure!(crates_packages.len() == 1, "crates.io package set");
    anyhow::ensure!(private_packages.len() == 2, "private registry package set");
    anyhow::ensure!(
        json.get("plan_id") == json.pointer("/registries/0/plan_id"),
        "legacy top-level plan identifies first effective registry"
    );
    anyhow::ensure!(
        json.pointer("/registries/0/plan_id") != json.pointer("/registries/1/plan_id"),
        "registry-specific package sets require distinct plan identities"
    );
    anyhow::ensure!(
        json.pointer("/outcome/next_action/reason")
            .and_then(serde_json::Value::as_str)
            == Some(EXPECTED_REASON),
        "exact registry-eligible JSON reason"
    );
    anyhow::ensure!(
        human_stdout
            .lines()
            .filter(|line| *line == format!("Next: {EXPECTED_REASON}"))
            .count()
            == 1,
        "exact registry-eligible human reason"
    );
    Ok(())
}

#[test]
fn status_completed_outcome_has_human_json_parity_without_side_effect_claims() -> Result<()> {
    struct Case {
        name: &'static str,
        statuses: Vec<u16>,
        expected_status: &'static str,
        expected_action: &'static str,
    }

    let cases = [
        Case {
            name: "all-published",
            statuses: vec![200, 200, 200],
            expected_status: "all_published",
            expected_action: "none_complete",
        },
        Case {
            name: "partially-published",
            statuses: vec![200, 404, 404],
            expected_status: "partially_published",
            expected_action: "preflight",
        },
        Case {
            name: "not-published",
            statuses: vec![404, 404, 404],
            expected_status: "not_published",
            expected_action: "preflight",
        },
    ];

    for case in cases {
        let temp = tempdir().context("create status parity workspace")?;
        create_multi_crate_workspace(temp.path());
        let secret = format!("status-secret-{}", case.name);
        let human_state = temp.path().join("human-state");
        let json_state = temp.path().join("json-state");
        let human = run_bounded_status(
            temp.path(),
            &human_state,
            case.statuses.clone(),
            "text",
            &secret,
        )?;
        let json_output =
            run_bounded_status(temp.path(), &json_state, case.statuses, "json", &secret)?;

        anyhow::ensure!(human.status.code() == Some(0), "{} human exit", case.name);
        anyhow::ensure!(
            json_output.status.code() == Some(0),
            "{} JSON exit",
            case.name
        );
        let human_stdout = String::from_utf8(human.stdout).context("human stdout UTF-8")?;
        let human_stderr = String::from_utf8(human.stderr).context("human stderr UTF-8")?;
        let json_stdout = String::from_utf8(json_output.stdout).context("JSON stdout UTF-8")?;
        let json_stderr = String::from_utf8(json_output.stderr).context("JSON stderr UTF-8")?;
        for stream in [&human_stdout, &human_stderr, &json_stdout, &json_stderr] {
            anyhow::ensure!(
                !stream.contains(&secret),
                "{} leaked token sentinel",
                case.name
            );
        }

        let json: serde_json::Value =
            serde_json::from_str(&json_stdout).context("parse status JSON")?;
        let legacy_schema = json_stdout
            .find("\"schema_version\"")
            .context("schema key")?;
        let legacy_plan = json_stdout.find("\"plan_id\"").context("plan key")?;
        let legacy_workspace = json_stdout
            .find("\"workspace_root\"")
            .context("workspace key")?;
        let legacy_registries = json_stdout
            .find("\"registries\"")
            .context("registries key")?;
        let additive_outcome = json_stdout.find("\"outcome\"").context("outcome key")?;
        anyhow::ensure!(
            legacy_schema < legacy_plan
                && legacy_plan < legacy_workspace
                && legacy_workspace < legacy_registries
                && legacy_registries < additive_outcome,
            "{} legacy JSON field order",
            case.name
        );
        anyhow::ensure!(
            json["schema_version"] == "shipper.status.v1",
            "legacy schema"
        );
        anyhow::ensure!(json.get("plan_id").is_some(), "legacy plan_id");
        anyhow::ensure!(
            json.get("workspace_root").is_some(),
            "legacy workspace_root"
        );
        anyhow::ensure!(json.get("registries").is_some(), "legacy registries");
        anyhow::ensure!(
            json.pointer("/outcome/status")
                .and_then(serde_json::Value::as_str)
                == Some(case.expected_status),
            "{} typed status",
            case.name
        );
        anyhow::ensure!(
            json.pointer("/outcome/publication_performed")
                .and_then(serde_json::Value::as_bool)
                == Some(false),
            "{} side-effect posture",
            case.name
        );
        anyhow::ensure!(
            json.pointer("/outcome/next_action/kind")
                .and_then(serde_json::Value::as_str)
                == Some(case.expected_action),
            "{} next action",
            case.name
        );
        anyhow::ensure!(
            json.pointer("/outcome/next_action/command").is_none(),
            "{} fabricated command",
            case.name
        );
        anyhow::ensure!(
            json.pointer("/outcome/safe_to_rerun").is_none(),
            "safety claim"
        );
        anyhow::ensure!(
            json.pointer("/outcome/evidence").is_none(),
            "evidence claim"
        );

        let reason = json
            .pointer("/outcome/next_action/reason")
            .and_then(serde_json::Value::as_str)
            .context("typed next-action reason")?;
        let expected_result = format!("Result: {}", case.expected_status.replace('_', " "));
        anyhow::ensure!(
            human_stdout
                .lines()
                .filter(|line| *line == expected_result)
                .count()
                == 1,
            "{} human result identity",
            case.name
        );
        anyhow::ensure!(
            human_stdout.contains("Publication performed: no"),
            "{} human side-effect posture",
            case.name
        );
        anyhow::ensure!(
            human_stdout.contains(&format!("Next: {reason}")),
            "{} human/JSON reason parity",
            case.name
        );
        anyhow::ensure!(
            !human_stdout.contains("Safe to rerun:"),
            "human safety claim"
        );
        anyhow::ensure!(!human_stdout.contains("Evidence:"), "human evidence claim");
        anyhow::ensure!(
            !human_state.exists(),
            "{} human state side effect",
            case.name
        );
        anyhow::ensure!(!json_state.exists(), "{} JSON state side effect", case.name);
        anyhow::ensure!(
            !temp.path().join(".shipper").exists(),
            "{} default state side effect",
            case.name
        );
    }

    Ok(())
}

#[test]
fn status_query_failure_has_no_completed_outcome_or_side_effects() -> Result<()> {
    let temp = tempdir().context("create status failure workspace")?;
    create_simple_workspace(temp.path());
    let secret = "status-query-secret-sentinel";
    let state_dir = temp.path().join("query-failure-state");
    let output = run_bounded_status(temp.path(), &state_dir, vec![500], "json", secret)?;
    anyhow::ensure!(output.status.code() == Some(1), "query failure exit");
    let stdout = String::from_utf8(output.stdout).context("failure stdout UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("failure stderr UTF-8")?;
    anyhow::ensure!(stdout.trim().is_empty(), "query failure JSON stdout");
    anyhow::ensure!(
        !stdout.contains("Result:"),
        "completed human outcome on stdout"
    );
    anyhow::ensure!(
        !stderr.contains("Result:"),
        "completed human outcome on stderr"
    );
    anyhow::ensure!(
        !stdout.contains(secret) && !stderr.contains(secret),
        "secret sentinel"
    );
    anyhow::ensure!(!state_dir.exists(), "query failure state side effect");
    anyhow::ensure!(
        !temp.path().join(".shipper").exists(),
        "default state side effect"
    );
    Ok(())
}

#[test]
fn status_output_format_name_at_version_colon_status() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    let registry = spawn_registry(vec![404], 1);

    let output = loopback_shipper_cmd()
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

    let output = loopback_shipper_cmd()
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
