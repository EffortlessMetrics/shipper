//! Integration tests verifying that config loading flows through to CLI behavior.
//!
//! Tests cover: default config, workspace .shipper.toml discovery, --config flag,
//! config values affecting behavior, invalid config errors, and CLI-flag precedence.

use std::fs;
use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;
use predicates::str::contains;
use serial_test::serial;
use tempfile::tempdir;

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

/// Create a minimal workspace with a single crate.
fn create_workspace(root: &Path) {
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

const CONFIG_SECRET: &str = "ISSUE_312_CONFIG_SECRET";

fn assert_secret_absent_and_no_execution_state(
    output: &std::process::Output,
    workspace: &Path,
    state_dir: &Path,
) {
    for (surface, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let rendered = String::from_utf8_lossy(bytes);
        assert!(
            !rendered.contains(CONFIG_SECRET),
            "secret leaked in {surface}: {rendered}"
        );
    }
    assert!(
        !workspace.join(".shipper").exists(),
        "config validation must not create default execution state"
    );
    assert!(
        !state_dir.exists(),
        "config validation must not create explicit execution state"
    );
}

fn run_plan_with_config(
    workspace: &Path,
    config: &Path,
    state_dir: &Path,
    allow_loopback: bool,
    registries: &str,
) -> std::process::Output {
    let mut command = shipper_cmd();
    command
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(workspace.join("Cargo.toml"))
        .arg("--config")
        .arg(config)
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--registries")
        .arg(registries);
    if allow_loopback {
        command.arg("--allow-loopback");
    }
    command
        .arg("plan")
        .env("ISSUE_312_REGISTRY_TOKEN", CONFIG_SECRET)
        .assert()
        .get_output()
        .clone()
}

fn write_two_loopback_registries(path: &Path) {
    write_file(
        path,
        r#"
schema_version = "shipper.config.v1"

[[registries.registries]]
name = "alpha"
api_base = "http://127.0.0.1:41001"
index_base = "http://127.0.0.1:41001"
token = "env:ISSUE_312_REGISTRY_TOKEN"

[[registries.registries]]
name = "beta"
api_base = "http://127.0.0.1:41002"
index_base = "http://127.0.0.1:41002"
token = "env:ISSUE_312_REGISTRY_TOKEN"
"#,
    );
}

// ── 1. CLI uses default config when no .shipper.toml exists ─────────

#[test]
fn plan_succeeds_without_config_file() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // No .shipper.toml present — CLI should use built-in defaults.
    assert!(
        !td.path().join(".shipper.toml").exists(),
        "precondition: no config file"
    );

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0"))
        .stdout(contains("Total packages to publish: 1"));
}

// ── 2. CLI reads .shipper.toml from workspace root ──────────────────

#[test]
fn plan_loads_config_from_workspace_root() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // Place a valid .shipper.toml in the workspace root.
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
schema_version = "shipper.config.v1"

[policy]
mode = "fast"
"#,
    );

    // The plan command should succeed — config is loaded and valid.
    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0"));
}

// ── 3. CLI --config flag overrides default path ─────────────────────

#[test]
fn config_flag_loads_from_custom_path() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // Put a valid config at a non-default location.
    let custom_config = td.path().join("custom").join("my-config.toml");
    write_file(
        &custom_config,
        r#"
schema_version = "shipper.config.v1"

[policy]
mode = "balanced"
"#,
    );

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&custom_config)
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0"));
}

#[test]
fn config_flag_with_missing_file_fails() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let missing = td.path().join("does-not-exist.toml");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&missing)
        .arg("plan")
        .assert()
        .failure()
        .stderr(contains("Failed to load config from"));
}

#[test]
#[serial]
fn two_configured_loopback_registries_require_explicit_flag() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let config = td.path().join("two-loopback.toml");
    write_two_loopback_registries(&config);

    let denied_state = td.path().join("state-denied");
    let denied = run_plan_with_config(td.path(), &config, &denied_state, false, "alpha,beta");
    assert_eq!(denied.status.code(), Some(1));
    let denied_stderr = String::from_utf8_lossy(&denied.stderr);
    assert!(
        denied_stderr.contains("plain http is reserved for an explicit loopback"),
        "{denied_stderr}"
    );
    assert_secret_absent_and_no_execution_state(&denied, td.path(), &denied_state);

    // Loopback trust does not broaden plan's single-target output contract.
    let multiple_state = td.path().join("state-multiple");
    let multiple = run_plan_with_config(td.path(), &config, &multiple_state, true, "alpha,beta");
    assert_eq!(multiple.status.code(), Some(1));
    assert!(multiple.stdout.is_empty(), "refusal must not emit a plan");
    let stderr = String::from_utf8_lossy(&multiple.stderr);
    assert!(
        stderr.contains("plan supports one registry at a time"),
        "{stderr}"
    );
    assert!(stderr.contains("--registries alpha"), "{stderr}");
    assert_secret_absent_and_no_execution_state(&multiple, td.path(), &multiple_state);

    // Follow the refusal's guidance and select each configured target alone.
    for registry in ["alpha", "beta"] {
        let allowed_state = td.path().join(format!("state-allowed-{registry}"));
        let allowed = run_plan_with_config(td.path(), &config, &allowed_state, true, registry);
        assert_eq!(allowed.status.code(), Some(0));
        assert!(allowed.stderr.is_empty(), "plan stderr must stay empty");
        let stdout = String::from_utf8_lossy(&allowed.stdout);
        assert!(stdout.contains("alpha@0.1.0"), "{stdout}");
        assert!(
            stdout.contains(&format!("registry: {registry} (")),
            "{stdout}"
        );
        assert_secret_absent_and_no_execution_state(&allowed, td.path(), &allowed_state);
    }
}

#[test]
fn scoped_registry_trust_cannot_be_combined_with_all_registries() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let config = td.path().join("two-loopback.toml");
    write_two_loopback_registries(&config);
    let state_dir = td.path().join("state-conflicting-selectors");

    let output = shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&config)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--registry")
        .arg("alpha")
        .arg("--all-registries")
        .arg("--allow-loopback")
        .arg("plan")
        .env("ISSUE_312_REGISTRY_TOKEN", CONFIG_SECRET)
        .assert()
        .get_output()
        .clone();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
    assert!(stderr.contains("--registry"), "{stderr}");
    assert!(stderr.contains("--all-registries"), "{stderr}");
    assert_secret_absent_and_no_execution_state(&output, td.path(), &state_dir);
}

#[test]
fn config_validate_honors_explicit_loopback_flag() {
    let td = tempdir().expect("tempdir");
    let config = td.path().join("two-loopback.toml");
    write_two_loopback_registries(&config);

    let denied = shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("config")
        .arg("validate")
        .arg("--path")
        .arg(&config)
        .env("ISSUE_312_REGISTRY_TOKEN", CONFIG_SECRET)
        .assert()
        .get_output()
        .clone();
    assert_eq!(denied.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&denied.stderr)
            .contains("plain http is reserved for an explicit loopback")
    );
    assert_secret_absent_and_no_execution_state(&denied, td.path(), &td.path().join("state"));

    let allowed = shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--allow-loopback")
        .arg("config")
        .arg("validate")
        .arg("--path")
        .arg(&config)
        .env("ISSUE_312_REGISTRY_TOKEN", CONFIG_SECRET)
        .assert()
        .get_output()
        .clone();
    assert_eq!(allowed.status.code(), Some(0));
    assert!(allowed.stderr.is_empty());
    assert!(String::from_utf8_lossy(&allowed.stdout).contains("Configuration file is valid"));
    assert_secret_absent_and_no_execution_state(&allowed, td.path(), &td.path().join("state"));
}

#[test]
#[serial]
fn parse_only_cli_boundary_rejects_unsupported_schema() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    let config = td.path().join("unsupported-schema.toml");
    write_file(
        &config,
        r#"
schema_version = "shipper.config.v2"
"#,
    );
    let state_dir = td.path().join("state-unsupported-schema");
    let output = run_plan_with_config(td.path(), &config, &state_dir, true, "alpha,beta");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported"), "{stderr}");
    assert!(stderr.contains("shipper.config.v2"), "{stderr}");
    assert_secret_absent_and_no_execution_state(&output, td.path(), &state_dir);
}

#[test]
#[serial]
fn allow_loopback_does_not_authorize_other_unsafe_destinations() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    for (case, alpha_base, beta_base, expected) in [
        (
            "plain-public-http-first",
            "http://registry.example.com",
            "https://registry.example.com",
            "must use https",
        ),
        (
            "plain-public-http-second",
            "https://registry.example.com",
            "http://registry.example.com",
            "must use https",
        ),
        (
            "metadata-address-first",
            "https://169.254.169.254",
            "https://registry.example.com",
            "link-local or metadata-routed",
        ),
        (
            "metadata-address-second",
            "https://registry.example.com",
            "https://169.254.169.254",
            "link-local or metadata-routed",
        ),
    ] {
        let config = td.path().join(format!("{case}.toml"));
        write_file(
            &config,
            &format!(
                r#"
schema_version = "shipper.config.v1"

[[registries.registries]]
name = "alpha"
api_base = "{alpha_base}"
index_base = "{alpha_base}"

[[registries.registries]]
name = "beta"
api_base = "{beta_base}"
index_base = "{beta_base}"
"#
            ),
        );
        let state_dir = td.path().join(format!("state-{case}"));
        let output = run_plan_with_config(td.path(), &config, &state_dir, true, "alpha,beta");
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{case}: {stderr}");
        assert_secret_absent_and_no_execution_state(&output, td.path(), &state_dir);
    }
}

// ── 4. Config values affect CLI behavior ────────────────────────────

#[test]
#[serial]
fn config_registry_name_appears_in_plan_output() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // Set a custom registry name in the config.
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
schema_version = "shipper.config.v1"

[registry]
name = "my-private-registry"
api_base = "https://registry.example.com"
index_base = "https://index.registry.example.com"
"#,
    );

    // The plan output should show the registry name from the config.
    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("my-private-registry"))
        .stdout(contains("https://registry.example.com"));
}

// ── 5. Invalid config file causes CLI to report error ───────────────

#[test]
fn invalid_toml_in_workspace_config_fails() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // Write broken TOML to .shipper.toml.
    write_file(
        &td.path().join(".shipper.toml"),
        "this is not valid toml {{{{",
    );

    let state_dir = td.path().join("state-malformed");
    let output = shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("plan")
        .env("ISSUE_312_REGISTRY_TOKEN", CONFIG_SECRET)
        .assert()
        .get_output()
        .clone();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Failed to load config from workspace")
    );
    assert_secret_absent_and_no_execution_state(&output, td.path(), &state_dir);
}

#[test]
fn config_with_invalid_values_fails_validation() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // output.lines = 0 is invalid per validation rules.
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
schema_version = "shipper.config.v1"

[output]
lines = 0
"#,
    );

    let state_dir = td.path().join("state-invalid-values");
    let output = shipper_cmd()
        .timeout(Duration::from_secs(20))
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("plan")
        .env("ISSUE_312_REGISTRY_TOKEN", CONFIG_SECRET)
        .assert()
        .get_output()
        .clone();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("validation failed"));
    assert_secret_absent_and_no_execution_state(&output, td.path(), &state_dir);
}

#[test]
fn config_flag_with_invalid_toml_fails() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let bad_config = td.path().join("bad.toml");
    write_file(&bad_config, "not valid [[[");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--config")
        .arg(&bad_config)
        .arg("plan")
        .assert()
        .failure()
        .stderr(contains("Failed to load config from"));
}

// ── 6. Config precedence: CLI flags override .shipper.toml values ───

#[test]
#[serial]
fn cli_registry_flag_overrides_config_registry() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // Config specifies a custom registry.
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
schema_version = "shipper.config.v1"

[registry]
name = "config-registry"
api_base = "https://config.example.com"
index_base = "https://index.config.example.com"
"#,
    );

    // CLI --registry flag should win over the config value.
    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--registry")
        .arg("cli-registry")
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("cli-registry"));
}

#[test]
#[serial]
fn cli_api_base_flag_overrides_config_api_base() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    // Config specifies a custom api_base.
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
schema_version = "shipper.config.v1"

[registry]
name = "my-reg"
api_base = "https://config-api.example.com"
index_base = "https://index.config-api.example.com"
"#,
    );

    // CLI --api-base flag should override config.
    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--api-base")
        .arg("https://cli-api.example.com")
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("https://cli-api.example.com"));
}
