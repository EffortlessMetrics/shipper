use std::fs;
use std::path::Path;
use std::process;
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
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
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

// ── preflight on a clean workspace ──────────────────────────────────

#[test]
fn preflight_clean_workspace_succeeds() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // 2 requests: version_exists + check_new_crate for "alpha"
    let registry = spawn_registry(vec![404], 2);

    shipper_cmd()
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
        .stdout(contains("Preflight Report"))
        .stdout(contains("alpha"))
        .stdout(contains("Total packages: 1"));

    registry.join();
}

#[test]
fn preflight_clean_workspace_shows_summary() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 2);

    let out = shipper_cmd()
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
    assert!(stdout.contains("Already published: 0"));
    assert!(stdout.contains("New crates: 1"));
    assert!(stdout.contains("Dry-run passed: 1"));

    registry.join();
}

// ── preflight on a non-git directory ────────────────────────────────

#[test]
fn preflight_non_git_directory_fails_without_allow_dirty() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    // Without --allow-dirty, preflight checks git cleanliness.
    // A non-git temp directory should fail the git check.
    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--skip-ownership-check")
        .arg("preflight")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .failure()
        .stderr(contains("git"));
}

// ── preflight --package <name> ──────────────────────────────────────

#[test]
fn preflight_package_filter_selects_single_package() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // 2 requests for the single filtered package
    let registry = spawn_registry(vec![404], 2);

    let out = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--package")
        .arg("core-lib")
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
    assert!(stdout.contains("core-lib"));
    assert!(stdout.contains("Total packages: 1"));
    // Only one package row should appear in the table (mid-lib and top-app are filtered out)
    assert!(!stdout.contains("Total packages: 2"));
    assert!(!stdout.contains("Total packages: 3"));

    registry.join();
}

#[test]
fn preflight_package_filter_multiple_packages() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // 4 requests: 2 per package (version_exists + check_new_crate)
    let registry = spawn_registry(vec![404], 4);

    let out = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--package")
        .arg("core-lib")
        .arg("--package")
        .arg("mid-lib")
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
    assert!(stdout.contains("core-lib"));
    assert!(stdout.contains("mid-lib"));
    assert!(stdout.contains("Total packages: 2"));

    registry.join();
}

// ── preflight --skip-ownership-check ────────────────────────────────

#[test]
fn preflight_skip_ownership_check_succeeds_without_token() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 2);

    // With --skip-ownership-check, ownership failures are not reported
    shipper_cmd()
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
        .stdout(contains("Preflight Report"))
        .stdout(contains("Ownership verified: 0"));

    registry.join();
}

#[test]
fn preflight_strict_ownership_fails_without_token() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    // --strict-ownership without a token should fail
    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--allow-dirty")
        .arg("--strict-ownership")
        .arg("preflight")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .failure()
        .stderr(contains("strict ownership requested but no token found"));
}

// ── preflight with custom manifest path ─────────────────────────────

#[test]
fn preflight_custom_manifest_path() {
    let td = tempdir().expect("tempdir");
    let nested = td.path().join("nested").join("project");
    create_simple_workspace(&nested);
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 2);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(nested.join("Cargo.toml"))
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
        .stdout(contains("Preflight Report"))
        .stdout(contains("alpha"));

    registry.join();
}

// ── Error cases ─────────────────────────────────────────────────────

#[test]
fn preflight_no_workspace_fails() {
    let td = tempdir().expect("tempdir");
    // Write a non-workspace file so there's no Cargo.toml
    write_file(&td.path().join("README.md"), "not a workspace");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("preflight")
        .assert()
        .failure();
}

#[test]
fn preflight_invalid_manifest_path_fails() {
    shipper_cmd()
        .arg("--manifest-path")
        .arg("nonexistent/path/Cargo.toml")
        .arg("preflight")
        .assert()
        .failure();
}

#[test]
fn preflight_json_format_produces_valid_json() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 2);

    let out = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--format")
        .arg("json")
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
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(json["plan_id"].is_string());
    assert!(json["packages"].is_array());
    assert_eq!(json["packages"].as_array().unwrap().len(), 1);
    assert_eq!(json["packages"][0]["name"], "alpha");

    registry.join();
}

#[test]
fn preflight_writes_events_file() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 2);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--state-dir")
        .arg(".shipper")
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("preflight")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
        .assert()
        .success();

    let events_path = td.path().join(".shipper").join("events.jsonl");
    assert!(events_path.exists(), "events.jsonl should be created");
    let events = fs::read_to_string(&events_path).expect("read events");
    assert!(events.contains("preflight_started"));
    assert!(events.contains("preflight_complete"));

    registry.join();
}

#[test]
fn preflight_multi_crate_workspace_lists_all_packages() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // 6 requests: 2 per package (version_exists + check_new_crate) for 3 packages
    let registry = spawn_registry(vec![404], 6);

    let out = shipper_cmd()
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
    assert!(stdout.contains("core-lib"));
    assert!(stdout.contains("mid-lib"));
    assert!(stdout.contains("top-app"));
    assert!(stdout.contains("Total packages: 3"));

    registry.join();
}

#[test]
fn preflight_reports_already_published_packages() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // Return 200 to indicate the version already exists
    let registry = spawn_registry(vec![200], 2);

    let out = shipper_cmd()
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
    assert!(stdout.contains("Already published: 1"));

    registry.join();
}

// ── Helper: initialise a git repo and commit everything ─────────────

fn git_init_and_commit(root: &Path) {
    let run = |args: &[&str]| {
        let status = process::Command::new("git")
            .args(args)
            .current_dir(root)
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::null())
            .status()
            .expect("git command");
        assert!(status.success(), "git {:?} failed", args);
    };
    run(&["init"]);
    run(&["config", "user.email", "test@test.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["add", "."]);
    run(&["commit", "-m", "init"]);
}

// ── 1. --allow-dirty skips git cleanliness check ────────────────────

#[test]
fn preflight_allow_dirty_skips_git_check() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    git_init_and_commit(td.path());

    // Dirty the working tree after the initial commit
    write_file(
        &td.path().join("alpha/src/lib.rs"),
        "pub fn alpha() { /* dirty */ }\n",
    );

    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 2);

    // With --allow-dirty the dirty working tree is accepted
    shipper_cmd()
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
        .stdout(contains("Preflight Report"))
        .stdout(contains("alpha"));

    registry.join();
}

// ── 2. --skip-ownership-check skips owner verification (token present) ─

#[test]
fn preflight_skip_ownership_with_token_present() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    // Only 2 requests expected (version_exists + check_new_crate).
    // If ownership were checked, the mock would need a 3rd request and the
    // thread would hang because spawn_registry only serves `expected_requests`.
    let registry = spawn_registry(vec![404], 2);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("preflight")
        .env("CARGO_HOME", td.path().join("cargo-home"))
        .env("CARGO_REGISTRY_TOKEN", "fake-token-for-test")
        .assert()
        .success()
        .stdout(contains("Token Detected: ✓"))
        .stdout(contains("Ownership verified: 0"));

    registry.join();
}

// ── 3. Preflight detects version already published per-package ──────

#[test]
fn preflight_detects_mixed_already_published_in_multi_crate() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    // Request order (dependency-sorted): core-lib, mid-lib, top-app
    // Each package: version_exists then check_new_crate
    //   core-lib: 200 (published), 200 (not new)
    //   mid-lib:  404 (not published), 404 (new)
    //   top-app:  404 (not published), 404 (new)
    let registry = spawn_registry(vec![200, 200, 404, 404, 404, 404], 6);

    let out = shipper_cmd()
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
    assert!(
        stdout.contains("Already published: 1"),
        "expected 'Already published: 1', got:\n{stdout}"
    );
    assert!(
        stdout.contains("New crates: 2"),
        "expected 'New crates: 2', got:\n{stdout}"
    );
    assert!(stdout.contains("Total packages: 3"));

    registry.join();
}

// ── 4. Preflight with custom registry URL ───────────────────────────

#[test]
fn preflight_custom_registry_url() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");
    let registry = spawn_registry(vec![404], 2);

    let out = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--registry")
        .arg("my-private-registry")
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
        .arg("--skip-ownership-check")
        .arg("--format")
        .arg("json")
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
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    // The preflight should resolve against the custom mock successfully
    assert!(json["plan_id"].is_string());
    assert_eq!(json["packages"].as_array().unwrap().len(), 1);
    assert_eq!(json["packages"][0]["name"], "alpha");
    // Verify the mock was used (package not already published on the custom registry)
    assert_eq!(json["packages"][0]["already_published"], false);
    assert_eq!(json["packages"][0]["is_new_crate"], true);

    registry.join();
}

// ── 5. Preflight reports multiple issues in a single run ────────────

#[test]
fn preflight_reports_multiple_issues_in_single_run() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());
    fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

    // core-lib: already published (200, 200)
    // mid-lib:  not published / new (404, 404)
    // top-app:  not published / new (404, 404)
    let registry = spawn_registry(vec![200, 200, 404, 404, 404, 404], 6);

    // No token → ownership cannot be verified for any package
    let out = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg(&registry.base_url)
        .arg("--allow-dirty")
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
    // Multiple findings are reported together in one summary
    assert!(
        stdout.contains("Already published: 1"),
        "expected 'Already published: 1', got:\n{stdout}"
    );
    assert!(
        stdout.contains("New crates: 2"),
        "expected 'New crates: 2', got:\n{stdout}"
    );
    assert!(
        stdout.contains("Ownership verified: 0"),
        "expected 'Ownership verified: 0', got:\n{stdout}"
    );
    // Dry-run failures + unverified ownership → finishability is FAILED
    assert!(
        stdout.contains("FAILED"),
        "expected finishability FAILED, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Dry-run Failures:"),
        "expected dry-run failure details, got:\n{stdout}"
    );

    registry.join();
}
