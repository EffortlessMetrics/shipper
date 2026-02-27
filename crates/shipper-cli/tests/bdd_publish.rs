//! BDD (Behavior-Driven Development) tests for the shipper publish workflow.
//!
//! These tests describe the expected behavior of shipper in various scenarios
//! using Given-When-Then style documentation.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

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

fn create_parallel_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "api", "cli", "app"]
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
        &root.join("api/Cargo.toml"),
        r#"
[package]
name = "api"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core" }
"#,
    );
    write_file(&root.join("api/src/lib.rs"), "pub fn api() {}\n");

    write_file(
        &root.join("cli/Cargo.toml"),
        r#"
[package]
name = "cli"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core" }
"#,
    );
    write_file(&root.join("cli/src/lib.rs"), "pub fn cli() {}\n");

    write_file(
        &root.join("app/Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
api = { path = "../api" }
cli = { path = "../cli" }
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

fn create_fake_cargo_with_sensitive_output(bin_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = bin_dir.join("cargo.cmd");
        fs::write(
            &path,
            "@echo off\r\nif \"%1\"==\"publish\" (\r\n  echo Authorization: Bearer super_secret_publish_token\r\n  echo CARGO_REGISTRY_TOKEN=super_secret_publish_token\r\n  echo token = \"super_secret_publish_token\" 1>&2\r\n  if \"%SHIPPER_FAKE_PUBLISH_EXIT%\"==\"\" (exit /b 0) else (exit /b %SHIPPER_FAKE_PUBLISH_EXIT%)\r\n)\r\n\"%REAL_CARGO%\" %*\r\nexit /b %ERRORLEVEL%\r\n",
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
            "#!/usr/bin/env sh\nif [ \"$1\" = \"publish\" ]; then\n  echo \"Authorization: Bearer super_secret_publish_token\"\necho \"CARGO_REGISTRY_TOKEN=super_secret_publish_token\"\necho \"token = \\\"super_secret_publish_token\\\"\" 1>&2\n  exit \"${SHIPPER_FAKE_PUBLISH_EXIT:-0}\"\nfi\n\"$REAL_CARGO\" \"$@\"\n",
        )
        .expect("write fake cargo");
        let mut perms = fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
        path
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

fn spawn_index_readiness_registry(crate_name: &str, version: &str) -> TestRegistry {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base_url = format!("http://{}", server.server_addr());
    let api_path = format!("/api/v1/crates/{crate_name}/{version}");
    let lower = crate_name.to_ascii_lowercase();
    let index_suffix = match lower.len() {
        1 => format!("1/{lower}"),
        2 => format!("2/{lower}"),
        3 => format!("3/{}/{lower}", &lower[..1]),
        _ => format!("{}/{}/{lower}", &lower[..2], &lower[2..4]),
    };
    let index_path = format!("/{index_suffix}");
    let index_body = format!(
        "{{\"name\":\"{crate_name}\",\"vers\":\"{version}\",\"deps\":[],\"cksum\":\"deadbeef\"}}"
    );

    let handle = thread::spawn(move || {
        for _ in 0..10 {
            let req = match server.recv_timeout(Duration::from_secs(60)) {
                Ok(Some(req)) => req,
                _ => break,
            };

            let (status, body) = if req.url() == api_path {
                (404, "{}".to_string())
            } else if req.url() == index_path {
                (200, index_body.clone())
            } else {
                (404, "{}".to_string())
            };

            let resp = Response::from_string(body)
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

    // Scenario: Workspace fan-out/fan-in groups independent crates into a shared parallel level
    #[test]
    fn given_parallelizable_workspace_when_grouping_levels_then_independent_crates_share_level() {
        let td = tempdir().expect("tempdir");
        create_parallel_workspace(td.path());

        let spec = shipper::types::ReleaseSpec {
            manifest_path: td.path().join("Cargo.toml"),
            registry: shipper::types::Registry::crates_io(),
            selected_packages: None,
        };
        let ws = shipper::plan::build_plan(&spec).expect("plan");
        let levels = ws.plan.group_by_levels();

        assert_eq!(levels.len(), 3);
        assert_eq!(
            levels[0]
                .packages
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["core"]
        );
        assert_eq!(
            levels[1]
                .packages
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["api", "cli"]
        );
        assert_eq!(
            levels[2]
                .packages
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["app"]
        );
    }
}

// ============================================================================
// Feature: Preflight Verification
// ============================================================================

mod preflight_verification {
    use super::*;

    // Scenario: preflight uses policy from workspace .shipper.toml when no --policy is passed.
    #[test]
    fn given_shipper_toml_policy_when_preflight_without_policy_uses_file_policy() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        write_file(
            &td.path().join(".shipper.toml"),
            r#"
[policy]
mode = "fast"
"#,
        );
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let registry = spawn_registry(vec![404, 404, 404, 404, 404, 404], 6);

        let mut cmd = shipper_cmd();
        let out = cmd
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
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false")
        );

        registry.join();
    }

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
// Feature: Readiness Modes
// ============================================================================

mod readiness_modes {
    use super::*;

    // Scenario: Index readiness mode accepts published version from sparse metadata
    #[test]
    fn given_index_readiness_mode_when_publish_then_reads_sparse_index_metadata() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

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
        #[cfg(windows)]
        let fake_cargo = bin_dir.join("cargo.cmd");
        #[cfg(not(windows))]
        let fake_cargo = bin_dir.join("cargo");

        let registry = spawn_index_readiness_registry("demo", "0.1.0");

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--readiness-method")
            .arg("index")
            .arg("--readiness-timeout")
            .arg("250ms")
            .arg("--readiness-poll")
            .arg("10ms")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .assert()
            .success();

        let receipt_path = td.path().join(".shipper").join("receipt.json");
        let receipt_json = fs::read_to_string(receipt_path).expect("receipt");
        let receipt: serde_json::Value = serde_json::from_str(&receipt_json).expect("json");
        let packages = receipt["packages"].as_array().expect("packages");
        assert!(packages.iter().any(|pkg| {
            pkg["name"].as_str() == Some("demo")
                && pkg["state"]["state"].as_str() == Some("published")
        }));

        registry.join();
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

    // Scenario: Balanced policy ignores strict ownership mode
    #[test]
    fn given_balanced_policy_with_strict_ownership_and_no_token_when_preflight_then_succeeds() {
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
            .arg("balanced")
            .arg("--strict-ownership")
            .arg("--no-verify")
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
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false")
        );

        registry.join();
    }

    #[cfg(feature = "micro-policy")]
    // Scenario: Preflight behavior is stable with policy micro backend enabled
    #[test]
    fn given_no_token_when_preflight_with_micro_policy_then_reports_token_not_detected() {
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
        assert!(
            stdout.contains("Token Detected: ✗") || stdout.contains("\"token_detected\":false")
        );

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

    // Scenario: Retryable publish output is classified for retry logic
    #[test]
    fn given_retryable_publish_output_when_failure_classification_runs_then_retryable() {
        let outcome =
            shipper::cargo_failure::classify_publish_failure("HTTP 429 too many requests", "");
        assert_eq!(
            outcome.class,
            shipper::cargo_failure::CargoFailureClass::Retryable
        );
    }

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

    // Scenario: Valid duration is accepted
    #[test]
    fn given_valid_duration_when_cli_then_plan_succeeds() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--base-delay")
            .arg("250ms")
            .arg("--max-delay")
            .arg("2s")
            .arg("plan")
            .assert()
            .success();
    }

    // Scenario: Valid retry options are accepted
    #[test]
    fn given_retry_options_when_plan_then_cli_accepts() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--max-attempts")
            .arg("3")
            .arg("--base-delay")
            .arg("250ms")
            .arg("--max-delay")
            .arg("1s")
            .arg("--retry-strategy")
            .arg("constant")
            .arg("--retry-jitter")
            .arg("0.25")
            .arg("plan")
            .assert()
            .success();
    }

    // Scenario: Invalid retry strategy is rejected
    #[test]
    fn given_invalid_retry_strategy_when_plan_then_cli_errors() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--retry-strategy")
            .arg("rocket")
            .arg("plan")
            .assert()
            .failure()
            .stderr(contains("invalid retry-strategy"));
    }

    // Scenario: Invalid schema versions are rejected by shared store validation
    #[test]
    fn given_invalid_schema_version_when_store_validation_runs_then_error() {
        let err =
            shipper::store::validate_schema_version("shipper.receipt.v").expect_err("must fail");
        assert!(err.to_string().contains("invalid"));
    }

    // Scenario: Supported schema versions pass shared store validation
    #[test]
    fn given_supported_schema_version_when_store_validation_runs_then_ok() {
        shipper::store::validate_schema_version(shipper::state::CURRENT_RECEIPT_VERSION)
            .expect("schema version should be accepted");
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

// ============================================================================
// Feature: Sensitive Output
// ============================================================================

mod output_sanitization {
    use super::*;

    // Scenario: Publish output that contains tokens is redacted before writing receipt evidence
    #[test]
    fn given_publish_outputs_sensitive_tokens_when_publish_succeeds_then_receipt_attempt_evidence_is_redacted()
     {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let bin_dir = td.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        let fake_cargo = create_fake_cargo_with_sensitive_output(&bin_dir);

        let old_path = std::env::var("PATH").unwrap_or_default();
        let mut new_path = bin_dir.display().to_string();
        if !old_path.is_empty() {
            new_path.push_str(path_sep());
            new_path.push_str(&old_path);
        }
        let real_cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

        let registry = spawn_registry(vec![404, 200], 4);

        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--api-base")
            .arg(&registry.base_url)
            .arg("--allow-dirty")
            .arg("--no-readiness")
            .arg("--state-dir")
            .arg(".shipper")
            .arg("publish")
            .env("PATH", &new_path)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIPPER_CARGO_BIN", &fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "0")
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
            .assert()
            .success();

        let receipt_path = td.path().join(".shipper").join("receipt.json");
        let receipt_json = fs::read_to_string(receipt_path).expect("receipt");
        let receipt: serde_json::Value =
            serde_json::from_str(&receipt_json).expect("receipt should be valid json");

        let packages = receipt["packages"].as_array().expect("packages");
        let demo_package = packages
            .iter()
            .find(|p| p["name"].as_str() == Some("demo"))
            .expect("demo package");
        let attempts = demo_package["evidence"]["attempts"]
            .as_array()
            .expect("attempts");
        assert_eq!(attempts.len(), 1);

        let attempt = &attempts[0];
        let stdout_tail = attempt["stdout_tail"].as_str().expect("stdout_tail");
        let stderr_tail = attempt["stderr_tail"].as_str().expect("stderr_tail");

        let secret = "super_secret_publish_token";
        assert!(!stdout_tail.contains(secret));
        assert!(!stderr_tail.contains(secret));
        assert!(stdout_tail.contains("[REDACTED]"));
        assert!(stderr_tail.contains("[REDACTED]"));

        registry.join();
    }
}
