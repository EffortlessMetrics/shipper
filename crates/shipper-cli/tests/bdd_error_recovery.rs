//! BDD (Behavior-Driven Development) tests for error recovery and retry scenarios.
//!
//! These tests describe the expected behavior of shipper when encountering
//! failures, corrupted files, and retry/resume workflows using Given-When-Then
//! style documentation.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;

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

// ============================================================================
// Feature: Failure Classification
// ============================================================================

mod failure_classification {

    // Scenario: Retryable failure is classified correctly by the library
    #[test]
    fn given_retryable_stderr_when_classifying_then_retryable() {
        // Given: cargo publish output containing a retryable pattern (rate limit)
        let outcome =
            shipper_core::cargo_failure::classify_publish_failure("HTTP 429 too many requests", "");

        // Then: The failure is classified as retryable
        assert_eq!(
            outcome.class,
            shipper_core::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: Permanent failure is classified correctly by the library
    #[test]
    fn given_permanent_stderr_when_classifying_then_permanent() {
        // Given: cargo publish output containing a permanent failure pattern
        let outcome =
            shipper_core::cargo_failure::classify_publish_failure("permission denied", "");

        // Then: The failure is classified as permanent
        assert_eq!(
            outcome.class,
            shipper_core::cargo_failure::CargoFailureClass::Permanent
        );
    }

    // Scenario: Ambiguous failure when no known pattern matches
    #[test]
    fn given_unknown_error_when_classifying_then_ambiguous() {
        // Given: cargo publish output with no recognizable pattern
        let outcome = shipper_core::cargo_failure::classify_publish_failure(
            "something completely unexpected happened",
            "",
        );

        // Then: The failure is classified as ambiguous
        assert_eq!(
            outcome.class,
            shipper_core::cargo_failure::CargoFailureClass::Ambiguous
        );
    }

    // Scenario: Retryable patterns take precedence over permanent patterns
    #[test]
    fn given_both_retryable_and_permanent_patterns_when_classifying_then_retryable_wins() {
        // Given: cargo publish output containing both retryable and permanent patterns
        let outcome = shipper_core::cargo_failure::classify_publish_failure(
            "permission denied and 429 too many requests",
            "",
        );

        // Then: Retryable takes precedence
        assert_eq!(
            outcome.class,
            shipper_core::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: Network timeout patterns are classified as retryable
    #[test]
    fn given_timeout_error_when_classifying_then_retryable() {
        let outcome = shipper_core::cargo_failure::classify_publish_failure(
            "operation timed out after 30s",
            "",
        );
        assert_eq!(
            outcome.class,
            shipper_core::cargo_failure::CargoFailureClass::Retryable
        );
    }

    // Scenario: Authentication failures are classified as permanent
    #[test]
    fn given_auth_failure_when_classifying_then_permanent() {
        let outcome = shipper_core::cargo_failure::classify_publish_failure("401 unauthorized", "");
        assert_eq!(
            outcome.class,
            shipper_core::cargo_failure::CargoFailureClass::Permanent
        );
    }
}

// ============================================================================
// Feature: State Persistence After Failure
// ============================================================================

mod state_persistence_after_failure {
    use super::*;

    // Scenario: Resume with corrupted state file reports parse error
    #[test]
    fn given_corrupted_state_file_when_resume_then_reports_parse_error() {
        // Given: A workspace with a corrupted state.json in the state directory
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("custom-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("state.json"), "NOT VALID JSON {{{").expect("write corrupt state");

        // When: We run shipper resume
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It fails with a JSON parse error
            .assert()
            .failure()
            .stderr(contains("failed to parse state JSON"));
    }

    // Scenario: State file with wrong plan_id is rejected on resume
    #[test]
    fn given_state_with_mismatched_plan_id_when_publish_then_rejected() {
        // Given: A workspace and a state file with a different plan_id
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("custom-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");

        let state_json = serde_json::json!({
            "state_version": "shipper.state.v1",
            "plan_id": "wrong-plan-id-12345",
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "packages": {}
        });
        fs::write(
            state_dir.join("state.json"),
            serde_json::to_string_pretty(&state_json).expect("serialize"),
        )
        .expect("write state");

        // When: We run shipper publish (which will load existing state)
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--allow-dirty")
            .arg("publish")
            // Then: It fails because the plan_id doesn't match
            .assert()
            .failure()
            .stderr(contains("does not match"));
    }

    // Scenario: Empty state file is treated as invalid JSON
    #[test]
    fn given_empty_state_file_when_resume_then_reports_parse_error() {
        // Given: A workspace with an empty state.json
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("empty-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("state.json"), "").expect("write empty state");

        // When: We run shipper resume
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It fails with a parse error
            .assert()
            .failure()
            .stderr(contains("failed to parse state JSON"));
    }
}

// ============================================================================
// Feature: Resume After Failure Skips Completed Packages
// ============================================================================

mod resume_skips_completed {
    use super::*;

    // Scenario: Resume with state showing a published package skips it
    #[test]
    fn given_state_with_published_package_when_resume_then_skips_completed() {
        // Given: A multi-crate workspace where core is already published in state
        let td = tempdir().expect("tempdir");
        create_multi_crate_workspace(td.path());

        // Use the library directly to get the deterministic plan_id
        let spec = shipper_core::types::ReleaseSpec {
            manifest_path: td.path().join("Cargo.toml"),
            registry: shipper_core::types::Registry::crates_io(),
            selected_packages: None,
        };
        let ws = shipper_core::plan::build_plan(&spec).expect("build plan");
        let plan_id = &ws.plan.plan_id;

        let state_dir = td.path().join("custom-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");

        // Write state with "core" already published
        let state_json = serde_json::json!({
            "state_version": "shipper.state.v1",
            "plan_id": plan_id,
            "registry": {
                "name": "crates-io",
                "api_base": "https://crates.io",
                "index_base": "https://index.crates.io"
            },
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "packages": {
                "core@0.1.0": {
                    "name": "core",
                    "version": "0.1.0",
                    "attempts": 1,
                    "state": { "state": "published" },
                    "last_updated_at": "2024-01-01T00:00:00Z"
                },
                "utils@0.1.0": {
                    "name": "utils",
                    "version": "0.1.0",
                    "attempts": 0,
                    "state": { "state": "pending" },
                    "last_updated_at": "2024-01-01T00:00:00Z"
                }
            }
        });
        fs::write(
            state_dir.join("state.json"),
            serde_json::to_string_pretty(&state_json).expect("serialize"),
        )
        .expect("write state");

        // When: We run shipper resume (it will fail eventually trying to publish
        // to crates.io, but the key behavior is that it skips the published package)
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--allow-dirty")
            .arg("resume")
            .output()
            .expect("resume");

        let stderr = String::from_utf8_lossy(&output.stderr);

        // Then: The output indicates core was already complete (skipped)
        assert!(
            stderr.contains("already complete") || stderr.contains("already published"),
            "expected 'already complete' or 'already published' in stderr, got:\n{stderr}"
        );
    }

    // Scenario: No state file means resume fails
    #[test]
    fn given_no_state_file_when_resume_then_fails() {
        // Given: A valid workspace with no prior publish state
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("empty-state");
        fs::create_dir_all(&state_dir).expect("mkdir");

        // When: We run shipper resume
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It fails because there's no state to resume from
            .assert()
            .failure()
            .stderr(contains("no existing state found"));
    }
}

// ============================================================================
// Feature: Invalid State File Handling
// ============================================================================

mod invalid_state_handling {
    use super::*;

    // Scenario: State file with invalid JSON structure (valid JSON but wrong schema)
    #[test]
    fn given_state_with_wrong_schema_when_resume_then_fails_gracefully() {
        // Given: A workspace and a state file that is valid JSON but wrong structure
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("bad-schema-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(
            state_dir.join("state.json"),
            r#"{"unexpected": "schema", "not": "execution_state"}"#,
        )
        .expect("write bad schema state");

        // When: We run shipper resume
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            .output()
            .expect("resume");

        // Then: It fails gracefully without a panic (non-zero exit, error on stderr)
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("failed to parse state JSON") || stderr.contains("missing field"),
            "expected parse/schema error in stderr, got:\n{stderr}"
        );
    }

    // Scenario: State file with truncated JSON
    #[test]
    fn given_truncated_state_json_when_resume_then_fails_gracefully() {
        // Given: A workspace with a truncated (incomplete) state.json
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("truncated-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(
            state_dir.join("state.json"),
            r#"{"state_version": "shipper.state.v1", "plan_id": "abc"#,
        )
        .expect("write truncated state");

        // When: We run shipper resume
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It reports a parse error without panicking
            .assert()
            .failure()
            .stderr(contains("failed to parse state JSON"));
    }

    // Scenario: State file containing a JSON array instead of object
    #[test]
    fn given_state_as_json_array_when_resume_then_fails_gracefully() {
        // Given: A state file that is a JSON array
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("array-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("state.json"), r#"[1, 2, 3]"#).expect("write array state");

        // When: We run shipper resume
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It fails with a parse error
            .assert()
            .failure()
            .stderr(contains("failed to parse state JSON"));
    }
}

// ============================================================================
// Feature: Corrupted Receipt File Handling
// ============================================================================

mod corrupted_receipt_handling {
    use super::*;

    // Scenario: Corrupted receipt.json doesn't crash inspect-receipt
    #[test]
    fn given_corrupted_receipt_when_inspect_receipt_then_fails_gracefully() {
        // Given: A workspace with a corrupted receipt.json
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("custom-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(
            state_dir.join("receipt.json"),
            "CORRUPTED RECEIPT DATA {{{}}}",
        )
        .expect("write corrupt receipt");

        // When: We run shipper inspect-receipt
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("inspect-receipt")
            .output()
            .expect("inspect-receipt");

        // Then: It fails with an error message (not a panic/crash)
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("failed to parse receipt") || stderr.contains("failed to read receipt"),
            "expected receipt parse error in stderr, got:\n{stderr}"
        );
    }

    // Scenario: Empty receipt.json doesn't crash the CLI
    #[test]
    fn given_empty_receipt_when_inspect_receipt_then_fails_gracefully() {
        // Given: A workspace with an empty receipt.json
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("empty-receipt-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("receipt.json"), "").expect("write empty receipt");

        // When: We run shipper inspect-receipt
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("inspect-receipt")
            .output()
            .expect("inspect-receipt");

        // Then: It fails gracefully
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("failed to parse receipt") || stderr.contains("failed to read receipt"),
            "expected receipt error in stderr, got:\n{stderr}"
        );
    }

    // Scenario: Receipt with wrong schema doesn't crash the CLI
    #[test]
    fn given_receipt_with_wrong_schema_when_inspect_receipt_then_fails_gracefully() {
        // Given: A workspace with a receipt that is valid JSON but wrong structure
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("wrong-schema-receipt");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("receipt.json"), r#"{"not": "a receipt"}"#)
            .expect("write bad receipt");

        // When: We run shipper inspect-receipt
        let output = shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("inspect-receipt")
            .output()
            .expect("inspect-receipt");

        // Then: It fails gracefully without panicking
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("failed to parse receipt") || stderr.contains("missing field"),
            "expected receipt parse error in stderr, got:\n{stderr}"
        );
    }

    // Scenario: Publish still works even if a corrupted receipt exists from a prior run
    #[test]
    fn given_corrupted_receipt_from_prior_run_when_plan_then_still_works() {
        // Given: A workspace with a leftover corrupted receipt.json
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join(".shipper");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("receipt.json"), "BROKEN LEFTOVER RECEIPT")
            .expect("write corrupt receipt");

        // When: We run shipper plan (which doesn't read receipt)
        shipper_cmd()
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("plan")
            // Then: Plan succeeds regardless of corrupted receipt
            .assert()
            .success();
    }
}
