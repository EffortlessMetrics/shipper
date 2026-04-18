//! Integration tests verifying that config loading flows through to CLI behavior.
//!
//! Tests cover: default config, workspace .shipper.toml discovery, --config flag,
//! config values affecting behavior, invalid config errors, and CLI-flag precedence.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::str::contains;
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

// ── 4. Config values affect CLI behavior ────────────────────────────

#[test]
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

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .failure()
        .stderr(contains("Failed to load config from workspace"));
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

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .failure()
        .stderr(contains("validation failed"));
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
