//! BDD (Behavior-Driven Development) tests for the shipper config commands.
//!
//! These tests describe the expected behavior of `shipper config init` and
//! `shipper config validate` using Given-When-Then style documentation.

use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

// ============================================================================
// Feature: Config Init
// ============================================================================

mod config_init {
    use super::*;

    // Scenario: Creating a default config file when none exists
    #[test]
    fn given_no_config_file_when_config_init_then_shipper_toml_is_created() {
        // Given: A temporary directory with no existing .shipper.toml
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join(".shipper.toml");
        assert!(!config_path.exists(), "precondition: no config file yet");

        // When: Running config init with --output pointing to the temp dir
        shipper_cmd()
            .args(["config", "init", "-o", config_path.to_str().unwrap()])
            .assert()
            .success()
            .stdout(contains("Created configuration file"));

        // Then: .shipper.toml is created with expected content
        assert!(config_path.exists(), ".shipper.toml should be created");
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

    // Scenario: Writing config to a custom path via --output
    #[test]
    fn given_config_init_with_output_then_file_is_written_to_specified_path() {
        // Given: A nested custom directory path
        let td = tempdir().expect("tempdir");
        let custom_path = td
            .path()
            .join("nested")
            .join("subdir")
            .join("my-config.toml");
        fs::create_dir_all(custom_path.parent().unwrap()).expect("mkdir");

        // When: Running config init with -o pointing to the custom path
        shipper_cmd()
            .args(["config", "init", "-o", custom_path.to_str().unwrap()])
            .assert()
            .success()
            .stdout(contains("Created configuration file"));

        // Then: The file is written at the specified path with valid content
        assert!(
            custom_path.exists(),
            "config file should exist at custom path"
        );
        let content = fs::read_to_string(&custom_path).expect("read");
        assert!(
            content.contains("schema_version"),
            "custom path config should contain schema_version"
        );
    }
}

// ============================================================================
// Feature: Config Validate
// ============================================================================

mod config_validate {
    use super::*;

    // Scenario: Validating a well-formed .shipper.toml succeeds
    #[test]
    fn given_valid_shipper_toml_when_config_validate_then_success_is_reported() {
        // Given: A valid .shipper.toml with all common sections
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
        .expect("write config");

        // When: Running config validate
        shipper_cmd()
            .args(["config", "validate", "-p", config_path.to_str().unwrap()])
            .assert()
            // Then: Success is reported
            .success()
            .stdout(contains("Configuration file is valid"));
    }

    // Scenario: Validating an invalid .shipper.toml reports errors
    #[test]
    fn given_invalid_shipper_toml_when_config_validate_then_errors_are_shown() {
        // Given: A .shipper.toml with invalid TOML syntax
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join("bad.toml");
        fs::write(&config_path, "this is not valid toml {{{{").expect("write");

        // When: Running config validate
        shipper_cmd()
            .args(["config", "validate", "-p", config_path.to_str().unwrap()])
            .assert()
            // Then: Failure is reported with an error message
            .failure()
            .stderr(contains("Failed to load config file"));
    }

    // Scenario: Validating with invalid field values reports errors
    #[test]
    fn given_invalid_values_when_config_validate_then_validation_errors_are_shown() {
        // Given: A .shipper.toml with an invalid output.lines value of 0
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join("invalid-values.toml");
        fs::write(
            &config_path,
            r#"
schema_version = "shipper.config.v1"

[output]
lines = 0
"#,
        )
        .expect("write");

        // When: Running config validate
        shipper_cmd()
            .args(["config", "validate", "-p", config_path.to_str().unwrap()])
            .assert()
            // Then: Validation failure is reported
            .failure()
            .stderr(contains("validation failed"));
    }

    // Scenario: Validating a file at a custom path via --path
    #[test]
    fn given_config_validate_with_path_then_specified_file_is_validated() {
        // Given: A valid config file at a deeply nested path
        let td = tempdir().expect("tempdir");
        let nested = td.path().join("deep").join("nested").join("config.toml");
        fs::create_dir_all(nested.parent().unwrap()).expect("mkdir");

        // First generate a valid config at the nested path
        shipper_cmd()
            .args(["config", "init", "-o", nested.to_str().unwrap()])
            .assert()
            .success();

        // When: Running config validate with -p pointing to the nested file
        shipper_cmd()
            .args(["config", "validate", "-p", nested.to_str().unwrap()])
            .assert()
            // Then: The specified file is validated successfully
            .success()
            .stdout(contains("Configuration file is valid"));
    }

    // Scenario: Validating an empty .shipper.toml succeeds with defaults
    #[test]
    fn given_empty_shipper_toml_when_config_validate_then_defaults_are_reported() {
        // Given: An empty .shipper.toml (all defaults apply)
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join(".shipper.toml");
        fs::write(&config_path, "").expect("write empty");

        // When: Running config validate
        shipper_cmd()
            .args(["config", "validate", "-p", config_path.to_str().unwrap()])
            .assert()
            // Then: Validation succeeds (defaults are valid)
            .success()
            .stdout(contains("Configuration file is valid"));
    }
}
