use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

// ── config init ──────────────────────────────────────────────────────

#[test]
fn config_init_creates_default_file() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("Created configuration file"));

    let content = fs::read_to_string(&config_path).expect("read config");
    assert!(
        content.contains("schema_version"),
        "generated config should contain schema_version"
    );
    assert!(
        content.contains("[policy]"),
        "generated config should contain [policy] section"
    );
}

#[test]
fn config_init_output_flag_writes_to_custom_path() {
    let td = tempdir().expect("tempdir");
    let custom = td.path().join("custom-dir").join("my-config.toml");

    // Parent directory must exist for fs::write to succeed
    fs::create_dir_all(custom.parent().unwrap()).expect("mkdir");

    shipper_cmd()
        .args(["config", "init", "-o", custom.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("Created configuration file"));

    assert!(
        custom.exists(),
        "config file should be written to custom path"
    );

    let content = fs::read_to_string(&custom).expect("read");
    assert!(content.contains("schema_version"));
}

#[test]
fn config_init_generated_file_is_valid() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    // Generate
    shipper_cmd()
        .args(["config", "init", "-o", config_path.to_str().unwrap()])
        .assert()
        .success();

    // Validate the generated file
    shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("Configuration file is valid"));
}

// ── config validate ──────────────────────────────────────────────────

#[test]
fn config_validate_valid_file() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    fs::write(
        &config_path,
        r#"
schema_version = "shipper.config.v1"

[policy]
mode = "safe"

[verify]
mode = "workspace"

[readiness]
enabled = true
method = "api"
initial_delay = "1s"
max_delay = "60s"
max_total_wait = "5m"
poll_interval = "2s"
jitter_factor = 0.5

[output]
lines = 50

[lock]
timeout = "1h"

[retry]
policy = "default"
max_attempts = 6
base_delay = "2s"
max_delay = "2m"
strategy = "exponential"
jitter = 0.5
"#,
    )
    .expect("write");

    shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("Configuration file is valid"));
}

#[test]
fn config_validate_invalid_toml_fails() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join("bad.toml");

    fs::write(&config_path, "this is not valid toml {{{{").expect("write");

    shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("Failed to load config file"));
}

#[test]
fn config_validate_missing_file_fails() {
    let td = tempdir().expect("tempdir");
    let missing = td.path().join("nonexistent.toml");

    shipper_cmd()
        .args(["config", "validate", "-p", missing.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("Config file not found"));
}

#[test]
fn config_validate_invalid_values_fails() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join("invalid-values.toml");

    // output.lines = 0 should fail validation
    fs::write(
        &config_path,
        r#"
schema_version = "shipper.config.v1"

[output]
lines = 0
"#,
    )
    .expect("write");

    shipper_cmd()
        .args(["config", "validate", "-p", config_path.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("validation failed"));
}

#[test]
fn config_validate_explicit_path_flag() {
    let td = tempdir().expect("tempdir");
    let nested = td.path().join("sub").join("dir").join("config.toml");
    fs::create_dir_all(nested.parent().unwrap()).expect("mkdir");

    // Generate a valid config at a nested path and validate it
    shipper_cmd()
        .args(["config", "init", "-o", nested.to_str().unwrap()])
        .assert()
        .success();

    shipper_cmd()
        .args(["config", "validate", "-p", nested.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("Configuration file is valid"));
}
