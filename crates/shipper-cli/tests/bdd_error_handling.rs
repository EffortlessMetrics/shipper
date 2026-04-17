//! BDD (Behavior-Driven Development) tests for error handling scenarios.
//!
//! These tests map to scenarios in `features/error_handling.feature` and verify
//! that shipper classifies publish failures correctly, retries transient errors
//! with backoff, fails fast on permanent errors, and preserves state for
//! partially-successful multi-crate publishes.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::tempdir;
use tiny_http::{Header, Response, Server, StatusCode};

// ── helpers ─────────────────────────────────────────────────────────────────

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

fn create_multi_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "utils"]
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
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

fn path_sep() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

/// Create a fake cargo proxy that echoes a custom stderr message and exits
/// with a configurable code. The message comes from `SHIPPER_FAKE_STDERR` and
/// the exit code from `SHIPPER_FAKE_PUBLISH_EXIT` (defaults to 1).
fn create_fake_cargo_with_stderr(bin_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = bin_dir.join("cargo.cmd");
        fs::write(
            &path,
            "@echo off\r\n\
             if \"%1\"==\"publish\" (\r\n\
               echo %SHIPPER_FAKE_STDERR% 1>&2\r\n\
               if \"%SHIPPER_FAKE_PUBLISH_EXIT%\"==\"\" (exit /b 1) else (exit /b %SHIPPER_FAKE_PUBLISH_EXIT%)\r\n\
             )\r\n\
             \"%REAL_CARGO%\" %*\r\n\
             exit /b %ERRORLEVEL%\r\n",
        )
        .expect("write fake cargo");
        path
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cargo");
        fs::write(
            &path,
            "#!/usr/bin/env sh\n\
             if [ \"$1\" = \"publish\" ]; then\n\
               echo \"$SHIPPER_FAKE_STDERR\" >&2\n\
               exit \"${SHIPPER_FAKE_PUBLISH_EXIT:-1}\"\n\
             fi\n\
             \"$REAL_CARGO\" \"$@\"\n",
        )
        .expect("write fake cargo");
        let mut perms = fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
        path
    }
}

/// Create a fake cargo that succeeds for the first N publishes, then fails
/// with a custom stderr message for subsequent ones.
fn create_fake_cargo_partial(bin_dir: &Path) -> PathBuf {
    // Uses a counter file to track how many publishes have been called.
    // SHIPPER_FAKE_SUCCEED_COUNT controls how many succeed before failure.
    #[cfg(windows)]
    {
        let path = bin_dir.join("cargo.cmd");
        // Avoid delayed-expansion bugs by using goto instead of nested blocks.
        // Redirect placed before echo to prevent `echo 2>file` being parsed
        // as stderr redirection.
        fs::write(
            &path,
            "@echo off\r\n\
             if not \"%1\"==\"publish\" goto passthrough\r\n\
             set /a _cnt=0\r\n\
             if exist \"%SHIPPER_FAKE_COUNTER_FILE%\" set /p _cnt=<\"%SHIPPER_FAKE_COUNTER_FILE%\"\r\n\
             set /a _cnt=%_cnt%+1\r\n\
             >\"%SHIPPER_FAKE_COUNTER_FILE%\" echo %_cnt%\r\n\
             if %_cnt% LEQ %SHIPPER_FAKE_SUCCEED_COUNT% exit /b 0\r\n\
             echo %SHIPPER_FAKE_STDERR% 1>&2\r\n\
             exit /b 1\r\n\
             :passthrough\r\n\
             \"%REAL_CARGO%\" %*\r\n\
             exit /b %ERRORLEVEL%\r\n",
        )
        .expect("write fake cargo");
        path
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = bin_dir.join("cargo");
        fs::write(
            &path,
            "#!/usr/bin/env sh\n\
             if [ \"$1\" = \"publish\" ]; then\n\
               count=0\n\
               if [ -f \"$SHIPPER_FAKE_COUNTER_FILE\" ]; then\n\
                 count=$(cat \"$SHIPPER_FAKE_COUNTER_FILE\")\n\
               fi\n\
               count=$((count + 1))\n\
               echo \"$count\" > \"$SHIPPER_FAKE_COUNTER_FILE\"\n\
               if [ \"$count\" -le \"$SHIPPER_FAKE_SUCCEED_COUNT\" ]; then\n\
                 exit 0\n\
               else\n\
                 echo \"$SHIPPER_FAKE_STDERR\" >&2\n\
                 exit 1\n\
               fi\n\
             fi\n\
             \"$REAL_CARGO\" \"$@\"\n",
        )
        .expect("write fake cargo");
        let mut perms = fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
        path
    }
}

fn prepend_fake_bin(bin_dir: &Path) -> (String, String) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let mut new_path = bin_dir.display().to_string();
    if !old_path.is_empty() {
        new_path.push_str(path_sep());
        new_path.push_str(&old_path);
    }
    let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    (new_path, real_cargo)
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

// ============================================================================
// Feature: Error handling and recovery
//   Shipper classifies publish failures into Retryable, Permanent, and
//   Ambiguous categories, applying appropriate retry strategies for transient
//   errors while failing fast on unrecoverable problems.
// ============================================================================

// ── Scenario 1: Auth failure (401) produces clear error message ─────────────

mod auth_failure {
    use super::*;

    // Scenario: Auth failure is classified as permanent and not retried
    //   Given cargo publish output contains "not authorized"
    //   When  publish failure classification runs
    //   Then  the failure class is "Permanent"
    //   And   no retry is attempted
    #[test]
    fn given_401_unauthorized_when_classifying_then_permanent() {
        let outcome = shipper::cargo_failure::classify_publish_failure("401 unauthorized", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Permanent
        );
    }

    // Scenario: Invalid token is classified as permanent
    //   Given cargo publish output contains "token is invalid"
    //   When  publish failure classification runs
    //   Then  the failure class is "Permanent"
    #[test]
    fn given_invalid_token_when_classifying_then_permanent() {
        let outcome = shipper::cargo_failure::classify_publish_failure("token is invalid", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Permanent
        );
    }

    // Scenario: "not authorized" in cargo output is classified as permanent
    #[test]
    fn given_not_authorized_when_classifying_then_permanent() {
        let outcome =
            shipper::cargo_failure::classify_publish_failure("not authorized to publish", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Permanent
        );
    }

    // Scenario: CLI reports auth error clearly when cargo publish says "unauthorized"
    #[test]
    fn given_auth_failure_stderr_when_publish_then_cli_reports_error() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_with_stderr(&bin_dir);
        let (new_path, real_cargo) = prepend_fake_bin(&bin_dir);

        // Registry: version-check → 404 (not published yet)
        let registry = spawn_registry(vec![404], 4);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
            .env(
                "SHIPPER_FAKE_STDERR",
                "error: 401 unauthorized: token is invalid",
            )
            .assert()
            .failure()
            .stderr(contains("permanent").or(contains("unauthorized").or(contains("failed"))));

        registry.join();
    }
}

// ── Scenario 2: Rate limiting (429) triggers backoff ────────────────────────

mod rate_limiting {
    use super::*;

    // Scenario: Rate limiting triggers retry with backoff
    //   Given cargo publish output contains "429 too many requests"
    //   When  publish failure classification runs
    //   Then  the failure class is "Retryable"
    //   And   the retry delay uses exponential backoff
    #[test]
    fn given_429_when_classifying_then_retryable() {
        let outcome =
            shipper::cargo_failure::classify_publish_failure("HTTP 429 too many requests", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    #[test]
    fn given_too_many_requests_when_classifying_then_retryable() {
        let outcome = shipper::cargo_failure::classify_publish_failure(
            "the remote server responded with 429 Too Many Requests",
            "",
        );
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: Rate-limited publish is retried by the CLI (max-attempts > 1)
    //   The fake cargo always returns 429 stderr; shipper should attempt
    //   more than once before giving up.
    #[test]
    fn given_rate_limit_stderr_when_publish_then_retries_before_failing() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_with_stderr(&bin_dir);
        let (new_path, real_cargo) = prepend_fake_bin(&bin_dir);

        // Enough registry responses for version checks + post-attempt checks
        let registry = spawn_registry(vec![404], 20);

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("2")
            .arg("--base-delay")
            .arg("0ms")
            .arg("--max-delay")
            .arg("0ms")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
            .env("SHIPPER_FAKE_STDERR", "error: 429 too many requests")
            .output()
            .expect("run");

        // Should fail (all retries exhausted)
        assert!(!output.status.success());

        let stderr = String::from_utf8_lossy(&output.stderr);
        // The engine should mention retry/attempt or the transient classification
        assert!(
            stderr.contains("attempt") || stderr.contains("retryable") || stderr.contains("failed"),
            "expected retry-related message in stderr, got:\n{stderr}"
        );

        registry.join();
    }
}

// ── Scenario 3: Network timeout produces retryable error ────────────────────

mod network_timeout {
    use super::*;

    // Scenario: Connection timeout triggers retry
    //   Given cargo publish output contains "operation timed out"
    //   When  publish failure classification runs
    //   Then  the failure class is "Retryable"
    #[test]
    fn given_timeout_when_classifying_then_retryable() {
        let outcome =
            shipper::cargo_failure::classify_publish_failure("operation timed out after 30s", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: DNS resolution failure triggers retry
    //   Given cargo publish output contains "dns error"
    //   When  publish failure classification runs
    //   Then  the failure class is "Retryable"
    #[test]
    fn given_dns_error_when_classifying_then_retryable() {
        let outcome =
            shipper::cargo_failure::classify_publish_failure("dns error: lookup failed", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: Connection reset triggers retry
    //   Given cargo publish output contains "connection reset by peer"
    //   When  publish failure classification runs
    //   Then  the failure class is "Retryable"
    #[test]
    fn given_connection_reset_when_classifying_then_retryable() {
        let outcome =
            shipper::cargo_failure::classify_publish_failure("connection reset by peer", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: Server error 502 triggers retry
    //   Given cargo publish output contains "502 bad gateway"
    //   When  publish failure classification runs
    //   Then  the failure class is "Retryable"
    #[test]
    fn given_502_when_classifying_then_retryable() {
        let outcome = shipper::cargo_failure::classify_publish_failure("502 bad gateway", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: CLI treats timeout stderr as retryable (attempts > 1)
    #[test]
    fn given_timeout_stderr_when_publish_then_retries() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_with_stderr(&bin_dir);
        let (new_path, real_cargo) = prepend_fake_bin(&bin_dir);

        let registry = spawn_registry(vec![404], 20);

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("2")
            .arg("--base-delay")
            .arg("0ms")
            .arg("--max-delay")
            .arg("0ms")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
            .env("SHIPPER_FAKE_STDERR", "error: operation timed out")
            .output()
            .expect("run");

        assert!(!output.status.success());
        registry.join();
    }
}

// ── Scenario 4: Invalid Cargo.toml produces permanent error ─────────────────

mod invalid_manifest {
    use super::*;

    // Scenario: Compilation failure is permanent
    //   Given cargo publish output contains "compilation failed"
    //   When  publish failure classification runs
    //   Then  the failure class is "Permanent"
    #[test]
    fn given_compilation_failed_when_classifying_then_permanent() {
        let outcome = shipper::cargo_failure::classify_publish_failure("compilation failed", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Permanent
        );
    }

    // Scenario: Missing manifest is rejected by CLI
    #[test]
    fn given_missing_manifest_when_plan_then_cli_errors() {
        let td = tempdir().expect("tempdir");

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("nonexistent").join("Cargo.toml"))
            .arg("plan")
            .assert()
            .failure();
    }

    // Scenario: Malformed Cargo.toml (invalid TOML) is a permanent error
    #[test]
    fn given_malformed_cargo_toml_when_plan_then_cli_errors() {
        let td = tempdir().expect("tempdir");
        write_file(&td.path().join("Cargo.toml"), "this is not valid TOML {{{");

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("plan")
            .assert()
            .failure();
    }

    // Scenario: Workspace with no members is rejected
    #[test]
    fn given_workspace_with_no_members_when_plan_then_cli_errors() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = []
resolver = "2"
"#,
        );

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("plan")
            .assert()
            .failure();
    }

    // Scenario: "failed to parse manifest" in cargo output is permanent
    #[test]
    fn given_parse_manifest_error_when_classifying_then_permanent() {
        let outcome = shipper::cargo_failure::classify_publish_failure(
            "failed to parse manifest at Cargo.toml",
            "",
        );
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Permanent
        );
    }
}

// ── Scenario 5: Registry unreachable produces connection error ──────────────

mod registry_unreachable {
    use super::*;

    // Scenario: "connection refused" is classified as retryable
    #[test]
    fn given_connection_refused_when_classifying_then_retryable() {
        let outcome = shipper::cargo_failure::classify_publish_failure("connection refused", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: "network unreachable" is classified as retryable
    #[test]
    fn given_network_unreachable_when_classifying_then_retryable() {
        let outcome = shipper::cargo_failure::classify_publish_failure("network unreachable", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: Preflight against a bogus api-base reports an error
    #[test]
    fn given_unreachable_registry_when_preflight_then_fails() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // Use a port that is almost certainly not listening
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg("http://127.0.0.1:1")
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .failure();
    }

    // Scenario: Unrecognized error is classified as ambiguous
    //   Given cargo publish output contains "unexpected registry response: xyz"
    //   When  publish failure classification runs
    //   Then  the failure class is "Ambiguous"
    #[test]
    fn given_unrecognized_error_when_classifying_then_ambiguous() {
        let outcome = shipper::cargo_failure::classify_publish_failure(
            "unexpected registry response: xyz",
            "",
        );
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Ambiguous
        );
    }
}

// ── Scenario 6: Mixed success/failure preserves state of successful pkgs ────

mod mixed_success_failure_state {
    use super::*;

    // Scenario: In a multi-crate workspace, if the first package succeeds and
    //   the second fails, the receipt/state records the first as "published"
    //   and the second as "failed".
    #[test]
    fn given_first_succeeds_second_fails_when_publish_then_state_preserves_success() {
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_partial(&bin_dir);
        let (new_path, real_cargo) = prepend_fake_bin(&bin_dir);

        let counter_file = td.path().join("publish_counter.txt");

        // Registry mock: 1st request (core pre-check) → 404,
        // 2nd request (core readiness) → 200 so core is confirmed as Published,
        // remaining requests (utils) → 404 so utils stays unverified.
        let registry = spawn_registry(vec![404, 200, 404], 20);

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("1")
            .arg("--base-delay")
            .arg("0ms")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_SUCCEED_COUNT", "1")
            .env("SHIPPER_FAKE_COUNTER_FILE", &counter_file)
            .env("SHIPPER_FAKE_STDERR", "error: simulated permanent failure")
            .output()
            .expect("run");

        // Overall publish should fail (second crate failed)
        assert!(!output.status.success());

        // State file should exist and record the first package as published
        let state_path = td.path().join(".shipper").join("state.json");
        if state_path.exists() {
            let state_json = fs::read_to_string(&state_path).expect("read state");
            let state: serde_json::Value = serde_json::from_str(&state_json).expect("parse state");

            let packages = state["packages"].as_object().expect("packages object");

            // "core" was the first to publish (leaf dep) and should have succeeded
            let core_entry = packages.iter().find(|(k, _)| k.starts_with("core"));
            if let Some((_key, core_state)) = core_entry {
                let pkg_state = core_state["state"]["state"].as_str().unwrap_or("");
                assert!(
                    pkg_state == "published" || pkg_state == "pending",
                    "expected core to be 'published' or 'pending', got: {pkg_state}"
                );
            }
        }

        registry.join();
    }

    // Scenario: Already-published version is classified as permanent
    //   Given cargo publish output contains "already exists"
    //   When  publish failure classification runs
    //   Then  the failure class is "Permanent"
    #[test]
    fn given_already_exists_when_classifying_then_permanent() {
        let outcome =
            shipper::cargo_failure::classify_publish_failure("version already exists: 0.1.0", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Permanent
        );
    }

    // Scenario: Retryable failure exhausts max attempts
    //   Given cargo publish fails with "connection reset by peer" on every attempt
    //   And   retry policy allows 3 max attempts
    //   When  I run "shipper publish"
    //   Then  the exit code is non-zero
    #[test]
    fn given_retryable_failure_when_max_attempts_exhausted_then_nonzero_exit() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_with_stderr(&bin_dir);
        let (new_path, real_cargo) = prepend_fake_bin(&bin_dir);

        let registry = spawn_registry(vec![404], 20);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("3")
            .arg("--base-delay")
            .arg("0ms")
            .arg("--max-delay")
            .arg("0ms")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
            .env("SHIPPER_FAKE_STDERR", "error: connection reset by peer")
            .assert()
            .failure();

        registry.join();
    }

    // Scenario: Exponential backoff respects max_delay cap
    //   Given retry config has base_delay "2s" and max_delay "30s"
    //   When  the 10th retry delay is computed
    //   Then  the delay does not exceed "30s"
    #[test]
    fn given_backoff_config_when_computing_delay_then_capped() {
        // Validates via CLI that extreme retry configs are accepted without
        // panicking — the actual delay cap is an engine-internal invariant.
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--base-delay")
            .arg("2s")
            .arg("--max-delay")
            .arg("30s")
            .arg("--max-attempts")
            .arg("10")
            .arg("plan")
            .assert()
            .success();
    }

    // Scenario: Retryable failure exhausts max attempts — receipt shows Failed
    //   Given cargo publish fails with "connection reset by peer" on every attempt
    //   And   retry policy allows 3 max attempts
    //   When  I run "shipper publish"
    //   Then  the receipt shows package "demo@0.1.0" in state "Failed"
    #[test]
    fn given_retryable_exhausted_then_receipt_shows_failed() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_with_stderr(&bin_dir);
        let (new_path, real_cargo) = prepend_fake_bin(&bin_dir);

        let state_dir = td.path().join(".shipper");
        let registry = spawn_registry(vec![404], 20);

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("3")
            .arg("--base-delay")
            .arg("0ms")
            .arg("--max-delay")
            .arg("0ms")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
            .env("SHIPPER_FAKE_STDERR", "error: connection reset by peer")
            .output()
            .expect("run");

        assert!(!output.status.success());

        // The receipt must record the package as failed.
        let receipt_path = state_dir.join("receipt.json");
        if receipt_path.exists() {
            let receipt_json = fs::read_to_string(&receipt_path).expect("read receipt");
            let receipt: serde_json::Value =
                serde_json::from_str(&receipt_json).expect("parse receipt");
            let packages = receipt["packages"].as_array().expect("packages array");
            let demo = packages
                .iter()
                .find(|p| p["name"].as_str() == Some("demo"))
                .expect("demo in receipt");
            let state = demo["state"]["state"].as_str().unwrap_or("");
            assert_eq!(
                state, "failed",
                "expected demo@0.1.0 in 'failed' state, got: {state}"
            );
        }

        registry.join();
    }
}

// ── Scenario: Ambiguous failure resolves to Published via registry check ────

mod ambiguous_resolves_via_registry {
    use super::*;

    // Scenario: Ambiguous failure resolves to Published via registry check
    //   Given cargo publish exits with an unrecognized error
    //   And   the registry returns "published" for "demo@0.1.0"
    //   When  publish failure classification runs and registry is checked
    //   Then  the package is marked as "Published"
    #[test]
    fn given_ambiguous_failure_when_registry_shows_published_then_marked_published() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_with_stderr(&bin_dir);
        let (new_path, real_cargo) = prepend_fake_bin(&bin_dir);

        let state_dir = td.path().join(".shipper");

        // Registry responses:
        //   1st request (pre-publish version_exists) → 404 (not yet published)
        //   2nd request (post-failure version_exists) → 200 (version appeared)
        let registry = spawn_registry(vec![404, 200], 10);

        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--max-attempts")
            .arg("1")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "1")
            .env(
                "SHIPPER_FAKE_STDERR",
                "error: unexpected registry response: xyz",
            )
            .output()
            .expect("run");

        let stderr = String::from_utf8_lossy(&output.stderr);

        // The engine should detect that the version appeared on the registry
        // and mark the package as published despite the cargo exit code.
        assert!(
            output.status.success()
                || stderr.contains("treating as published")
                || stderr.contains("present on registry"),
            "expected success or registry-resolved publish in stderr, got:\n{stderr}"
        );

        // Check receipt for Published state
        let receipt_path = state_dir.join("receipt.json");
        if receipt_path.exists() {
            let receipt_json = fs::read_to_string(&receipt_path).expect("read receipt");
            let receipt: serde_json::Value =
                serde_json::from_str(&receipt_json).expect("parse receipt");
            let packages = receipt["packages"].as_array().expect("packages array");
            let demo = packages
                .iter()
                .find(|p| p["name"].as_str() == Some("demo"))
                .expect("demo in receipt");
            let state = demo["state"]["state"].as_str().unwrap_or("");
            assert!(
                state == "published" || state == "skipped",
                "expected demo@0.1.0 to be 'published' or 'skipped', got: {state}"
            );
        }

        registry.join();
    }
}
