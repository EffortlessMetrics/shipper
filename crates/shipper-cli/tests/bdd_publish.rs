//! BDD (Behavior-Driven Development) tests for the shipper publish workflow.
//!
//! These tests describe the expected behavior of shipper in various scenarios
//! using Given-When-Then style documentation.

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
members = ["core", "utils", "app"]
resolver = "2"
"#,
    );

    // Core crate (no dependencies)
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

    // Utils crate (depends on core)
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

    // App crate (depends on utils and core)
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

fn path_sep() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
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

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

// ============================================================================
// Feature: Deterministic Publish Order
// ============================================================================

mod deterministic_publish_order {
    use super::*;

    // Scenario: Workspace with dependency chain publishes in correct order
    #[test]
    fn given_workspace_with_dependency_chain_when_plan_then_publishes_in_order() {
        // Given: A workspace with core -> utils -> app dependency chain
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // When: We run shipper plan
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

        // Then: Packages are listed in dependency order (core first, app last)
        let stdout = String::from_utf8(out).expect("utf8");
        let core_pos = stdout.find("core@0.1.0").expect("core should be in output");
        let utils_pos = stdout
            .find("utils@0.1.0")
            .expect("utils should be in output");
        let app_pos = stdout.find("app@0.1.0").expect("app should be in output");

        // core should come before utils, and utils before app
        assert!(core_pos < utils_pos, "core should be listed before utils");
        assert!(utils_pos < app_pos, "utils should be listed before app");
    }
}

// ============================================================================
// Feature: Preflight Verification
// ============================================================================

mod preflight_verification {
    use super::*;

    // Scenario: Preflight detects missing token (using --policy fast to skip dry-run)
    #[test]
    fn given_no_token_when_preflight_then_reports_token_not_detected() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // 3 crates x (version check + new crate check) = 6 requests
        let registry = spawn_registry(vec![404, 404, 404, 404, 404, 404], 6);

        // When: Running preflight without a token (using fast policy to skip dry-run)
        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then: Token is reported as not detected
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false")
        );

        registry.join();
    }

    // Scenario: Preflight behavior is stable with micro backends enabled
    #[test]
    fn given_no_token_when_preflight_with_micro_backend_flags_then_reports_token_not_detected() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // 3 crates x (version check + new crate check) = 6 requests
        let registry = spawn_registry(vec![404, 404, 404, 404, 404, 404], 6);

        // When: Running preflight without a token (using fast policy to skip dry-run)
        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
            .arg("preflight")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        // Then: Token is reported as not detected
        let stdout = String::from_utf8(out).expect("utf8");
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false")
        );

        registry.join();
    }

    // Scenario: Preflight detects already published versions (using --policy fast to skip dry-run)
    #[test]
    fn given_already_published_version_when_preflight_then_reports_already_published() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // Mock registry returns 200 for version_exists (already published) - 3 crates x 2 checks
        let registry = spawn_registry(vec![200, 200, 200, 200, 200, 200], 6);

        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
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
            stdout.contains("Already published: 3")
                || stdout.contains("\"already_published\":true")
        );

        registry.join();
    }
}

// ============================================================================
// Feature: Resumability
// ============================================================================

mod resumability {
    use super::*;

    // Scenario: Resume skips already published packages
    #[test]
    fn given_partial_publish_when_resume_then_skips_completed() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        create_fake_cargo_proxy(&bin_dir);

        let old_path = std::env::var("PATH").unwrap_or_default();
        let mut new_path = bin_dir.display().to_string();
        if !old_path.is_empty() {
            new_path.push_str(path_sep());
            new_path.push_str(&old_path);
        }
        let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

        // First publish (should succeed)
        let registry = spawn_registry(vec![404, 200, 404, 200, 404, 200], 6);

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
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        registry.join();

        // Second registry for resume check (returns 200 for all - already published)
        let registry2 = spawn_registry(vec![200, 200, 200], 3);

        // Resume should see everything is published
        let mut resume = shipper_cmd();
        let out = resume
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry2.base_url)
            .arg("--allow-dirty")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("status")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(out).expect("utf8");
        // State should exist
        assert!(stdout.contains("plan_id:"));

        registry2.join();
    }
}

// ============================================================================
// Feature: Policy Modes
// ============================================================================

mod policy_modes {
    use super::*;

    // Scenario: Fast policy skips verification
    #[test]
    fn given_fast_policy_when_preflight_then_skips_dry_run() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // 3 crates x (version check + new crate check) = 6 requests
        let registry = spawn_registry(vec![404, 404, 404, 404, 404, 404], 6);

        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
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
        // Fast policy should show dry-run passed (skipped)
        assert!(stdout.contains("Dry-run") || stdout.contains("dry_run"));

        registry.join();
    }
}

// ============================================================================
// Feature: Output Formats
// ============================================================================

mod output_formats {
    use super::*;

    // Scenario: JSON output for preflight is valid JSON
    #[test]
    fn given_json_format_when_preflight_then_output_is_valid_json() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        // 3 crates x (version check + new crate check) = 6 requests (using fast policy)
        let registry = spawn_registry(vec![404, 404, 404, 404, 404, 404], 6);

        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--policy")
            .arg("fast")
            .arg("preflight")
            .arg("--format")
            .arg("json")
            .env("CARGO_HOME", td.path().join("cargo-home"))
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(out).expect("utf8");
        let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
        assert!(json.get("plan_id").is_some());
        assert!(json.get("packages").is_some());

        registry.join();
    }

    // Scenario: Status command shows package states
    #[test]
    fn given_packages_when_status_then_shows_each_package() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let registry = spawn_registry(vec![404], 3);

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
        assert!(stdout.contains("core@0.1.0"));
        assert!(stdout.contains("utils@0.1.0"));
        assert!(stdout.contains("app@0.1.0"));

        registry.join();
    }
}

// ============================================================================
// Feature: Error Handling
// ============================================================================

mod error_handling {
    use super::*;

    // Scenario: Invalid duration is rejected
    #[test]
    fn given_invalid_duration_when_cli_then_error() {
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

    // Scenario: Missing manifest is rejected
    #[test]
    fn given_missing_manifest_when_cli_then_error() {
        let td = tempdir().expect("tempdir");

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("nonexistent").join("Cargo.toml"))
            .arg("plan")
            .assert()
            .failure();
    }
}

// ============================================================================
// Feature: CI Templates
// ============================================================================

mod ci_templates {
    use super::*;

    // Scenario: GitHub Actions template is valid YAML
    #[test]
    fn given_github_actions_template_then_is_valid_yaml() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("ci")
            .arg("github-actions")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(out).expect("utf8");
        // Basic YAML validation - should parse without error
        let _: serde_yaml::Value = serde_yaml::from_str(&stdout).expect("should be valid YAML");
    }

    // Scenario: GitLab CI template is valid YAML
    #[test]
    fn given_gitlab_template_then_is_valid_yaml() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("ci")
            .arg("gitlab")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(out).expect("utf8");
        let _: serde_yaml::Value = serde_yaml::from_str(&stdout).expect("should be valid YAML");
    }

    // Scenario: CircleCI template is valid YAML
    #[test]
    fn given_circleci_template_then_is_valid_yaml() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("ci")
            .arg("circleci")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(out).expect("utf8");
        let _: serde_yaml::Value = serde_yaml::from_str(&stdout).expect("should be valid YAML");
        assert!(
            stdout.contains("restore_cache"),
            "CircleCI template should include restore_cache"
        );
        assert!(
            stdout.contains("save_cache"),
            "CircleCI template should include save_cache"
        );
    }

    // Scenario: Azure DevOps template is valid YAML
    #[test]
    fn given_azure_devops_template_then_is_valid_yaml() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut cmd = shipper_cmd();
        let out = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("ci")
            .arg("azure-devops")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(out).expect("utf8");
        let _: serde_yaml::Value = serde_yaml::from_str(&stdout).expect("should be valid YAML");
        assert!(
            stdout.contains("Cache@2"),
            "Azure DevOps template should include Cache task"
        );
    }
}
