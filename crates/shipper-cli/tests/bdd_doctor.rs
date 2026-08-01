//! BDD (Behavior-Driven Development) tests for the shipper doctor command.
//!
//! These tests describe the expected behavior of `shipper doctor` in various
//! scenarios using Given-When-Then style documentation.

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

// ============================================================================
// Feature: Doctor Command - Environment Diagnostics
// ============================================================================

mod environment_info {
    use super::*;

    // Scenario: Doctor reports environment info for a clean workspace
    #[test]
    fn given_clean_environment_when_running_doctor_then_reports_environment_info() {
        // Given: A valid workspace and a reachable registry
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(1);

        // When: We run shipper doctor
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        // Then: Output includes diagnostic header, workspace root, and completion
        cmd.assert()
            .success()
            .stdout(contains("Shipper Doctor - Diagnostics Report"))
            .stdout(contains("workspace_root:"))
            .stdout(contains("Diagnostics complete."));

        registry.join();
    }
}

// ============================================================================
// Feature: Doctor Command - Cargo Version Detection
// ============================================================================

mod cargo_detection {
    use super::*;

    // Scenario: Doctor detects the installed cargo version
    #[test]
    fn given_cargo_is_installed_when_running_doctor_then_cargo_version_is_detected() {
        // Given: A valid workspace (cargo is assumed to be available on PATH)
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(1);

        // When: We run shipper doctor
        let mut cmd = shipper_cmd();
        let output = cmd
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

        // Then: The output includes a cargo version line
        let stdout = String::from_utf8(output).expect("utf8");
        assert!(
            stdout.contains("cargo: cargo"),
            "Expected cargo version line in output, got:\n{stdout}"
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Doctor Command - Missing Token Warning
// ============================================================================

mod missing_token {
    use super::*;

    // Scenario: Doctor warns when no registry token is found
    #[test]
    fn given_no_cargo_registry_token_when_running_doctor_then_warns_about_missing_token() {
        // Given: A workspace with no token in environment or credentials file
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let cargo_home = td.path().join("cargo-home");
        fs::create_dir_all(&cargo_home).expect("mkdir");

        let registry = spawn_registry(1);

        // When: We run shipper doctor with token env vars removed
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", &cargo_home)
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        // Then: Output indicates no authentication was found
        cmd.assert().success().stdout(contains("NONE FOUND"));

        registry.join();
    }
}

// ============================================================================
// Feature: Doctor Command - Workspace Health
// ============================================================================

mod workspace_health {
    use super::*;

    // Scenario: Doctor checks workspace health and reports registry and state info
    #[test]
    fn given_valid_workspace_when_running_doctor_then_checks_workspace_health() {
        // Given: A valid workspace and a reachable mock registry
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(1);

        // When: We run shipper doctor
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        // Then: Output shows registry info, state_dir, and tool versions
        cmd.assert()
            .success()
            .stdout(contains("registry:"))
            .stdout(contains("state_dir:"))
            .stdout(contains("registry_reachable: true"))
            .stdout(contains("git:"));

        registry.join();
    }

    // Scenario: Doctor reports state directory status when it does not yet exist
    #[test]
    fn given_nonexistent_state_dir_when_running_doctor_then_reports_will_be_created() {
        // Given: A workspace where the state dir has not been created yet
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(1);

        // When: We run shipper doctor
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        // Then: Output indicates the state directory will be created
        cmd.assert()
            .success()
            .stdout(contains("state_dir_exists: false (will be created)"));

        registry.join();
    }
}

// ============================================================================
// Feature: Doctor Command - Runs Without A Loadable Workspace
// ============================================================================

mod without_workspace {
    use super::*;

    // Scenario: Doctor still diagnoses the environment from a directory that
    // has no Cargo.toml at all.
    #[test]
    fn given_no_manifest_when_running_doctor_then_reports_environment_and_flags_missing_plan() {
        // Given: A directory with no workspace manifest
        let td = tempdir().expect("tempdir");
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(1);

        // When: We run shipper doctor against it
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        // Then: It succeeds, the environment checks still run, and the missing
        // plan is reported as a finding rather than as a fatal error.
        cmd.assert()
            .success()
            .stdout(contains("Shipper Doctor - Diagnostics Report"))
            .stdout(contains("no publish plan"))
            .stdout(contains("workspace-plan-unavailable"))
            .stdout(contains("registry_reachable: true"))
            .stdout(contains("Diagnostics complete."));

        registry.join();
    }

    // Scenario: The JSON envelope carries the same signal for CI consumers.
    #[test]
    fn given_no_manifest_when_running_doctor_json_then_envelope_marks_workspace_unavailable() {
        // Given: A directory with no workspace manifest
        let td = tempdir().expect("tempdir");
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(1);

        // When: We ask for the machine-readable report
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--format")
            .arg("json")
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        let output = cmd.assert().success().get_output().stdout.clone();
        let parsed: serde_json::Value =
            serde_json::from_slice(&output).expect("doctor JSON envelope parses");

        // Then: The schema is unchanged and the envelope names the missing
        // plan. Both live at envelope level, not inside a per-registry
        // report — see the multi-registry scenario below for why.
        assert_eq!(parsed["schema_version"], "shipper.doctor.v1");
        assert!(
            parsed["workspace_unavailable"].is_string(),
            "expected workspace_unavailable reason, got {parsed}"
        );
        let finding_ids: Vec<&str> = parsed["workspace_findings"]
            .as_array()
            .expect("workspace_findings array")
            .iter()
            .filter_map(|finding| finding["id"].as_str())
            .collect();
        assert_eq!(finding_ids, vec!["workspace-plan-unavailable"]);

        registry.join();
    }

    // Scenario: with several registries, the workspace condition is still
    // reported exactly once. It describes the run, not any registry, so a
    // consumer counting blockers must not see it multiplied by N.
    #[test]
    fn given_multiple_registries_when_running_doctor_json_then_workspace_finding_is_not_repeated() {
        // Given: A directory with no workspace manifest and two registries
        let td = tempdir().expect("tempdir");
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(2);
        write_file(
            &td.path().join(".shipper.toml"),
            &format!(
                r#"
[[registries.registries]]
name = "alpha"
api_base = "{base}"

[[registries.registries]]
name = "beta"
api_base = "{base}"
"#,
                base = registry.base_url
            ),
        );

        // When: We ask for diagnostics against both
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--all-registries")
            .arg("--format")
            .arg("json")
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        let output = cmd.assert().success().get_output().stdout.clone();
        let parsed: serde_json::Value =
            serde_json::from_slice(&output).expect("doctor JSON envelope parses");

        // Then: Two registry reports, one workspace finding.
        assert_eq!(
            parsed["reports"].as_array().expect("reports array").len(),
            2
        );
        assert_eq!(
            parsed["workspace_findings"]
                .as_array()
                .expect("workspace_findings array")
                .len(),
            1
        );
        for report in parsed["reports"].as_array().expect("reports array") {
            assert!(
                report["workspace_unavailable"].is_null(),
                "per-registry reports must not repeat the run-level condition: {report}"
            );
            let ids: Vec<&str> = report["findings"]
                .as_array()
                .expect("findings array")
                .iter()
                .filter_map(|finding| finding["id"].as_str())
                .collect();
            assert!(
                !ids.contains(&"workspace-plan-unavailable"),
                "workspace finding leaked into a registry report: {ids:?}"
            );
        }

        registry.join();
    }

    // Scenario: the text path applies the same rule as the JSON envelope —
    // the workspace header repeats per registry block (each block is read on
    // its own) but the finding is listed once, so the findings list stays a
    // list of distinct problems.
    #[test]
    fn given_multiple_registries_when_running_doctor_text_then_finding_listed_once() {
        // Given: A directory with no workspace manifest and two registries
        let td = tempdir().expect("tempdir");
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(2);
        write_file(
            &td.path().join(".shipper.toml"),
            &format!(
                r#"
[[registries.registries]]
name = "alpha"
api_base = "{base}"

[[registries.registries]]
name = "beta"
api_base = "{base}"
"#,
                base = registry.base_url
            ),
        );

        // When: We run text diagnostics against both
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--all-registries")
            .arg("doctor")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN");

        let output = cmd.assert().success().get_output().stdout.clone();
        let stdout = String::from_utf8_lossy(&output);

        // Then: Two diagnostic blocks, each noting the missing plan in its
        // header, but exactly one `workspace-plan-unavailable` finding.
        assert_eq!(
            stdout
                .matches("Shipper Doctor - Diagnostics Report")
                .count(),
            2,
            "expected one diagnostics block per registry:\n{stdout}"
        );
        assert_eq!(
            stdout.matches("(no publish plan").count(),
            2,
            "each block's header should carry the workspace note:\n{stdout}"
        );
        assert_eq!(
            stdout.matches("workspace-plan-unavailable").count(),
            1,
            "the finding itself must be listed once:\n{stdout}"
        );

        registry.join();
    }

    // Scenario: `shipper ci` prints a static snippet, so it must not need a
    // workspace either.
    #[test]
    fn given_no_manifest_when_printing_ci_snippet_then_it_still_prints() {
        // Given: A directory with no workspace manifest
        let td = tempdir().expect("tempdir");

        // When: We ask for the GitHub Actions snippet
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("ci")
            .arg("github-actions");

        // Then: The snippet is printed
        cmd.assert()
            .success()
            .stdout(contains("GitHub Actions workflow snippet"));
    }
}
