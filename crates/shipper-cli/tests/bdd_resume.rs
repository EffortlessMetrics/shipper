//! BDD (Behavior-Driven Development) tests for the shipper resume workflow.
//!
//! These tests describe the expected behavior of `shipper resume` in various
//! error scenarios using Given-When-Then style documentation.

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

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

// ============================================================================
// Feature: Resume requires existing state
// ============================================================================

mod resume_requires_state {
    use super::*;

    // Scenario: No state file exists in the default state directory
    #[test]
    fn given_no_state_file_when_resume_then_fails_with_no_state() {
        // Given: A valid workspace with no prior publish (no state.json)
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("custom-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");

        // When: We run shipper resume with an explicit --state-dir
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It fails because there is nothing to resume
            .assert()
            .failure()
            .stderr(contains("no existing state found"));
    }

    // Scenario: An empty state directory
    #[test]
    fn given_empty_state_dir_when_resume_then_reports_no_state_found() {
        // Given: A valid workspace and an empty state directory
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("empty-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");

        // When: We run shipper resume pointing to the empty dir
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It reports no state found
            .assert()
            .failure()
            .stderr(contains("no existing state found"));
    }

    // Scenario: A corrupted state file
    #[test]
    fn given_corrupted_state_file_when_resume_then_reports_parse_error() {
        // Given: A valid workspace and a state directory with a corrupted state.json
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("corrupt-state");
        fs::create_dir_all(&state_dir).expect("mkdir state dir");
        fs::write(state_dir.join("state.json"), "NOT VALID JSON {{{").expect("write corrupt state");

        // When: We run shipper resume
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It fails with a JSON parse error
            .assert()
            .failure()
            .stderr(contains("failed to parse state JSON"));
    }

    // Scenario: --state-dir points to a path that does not exist
    #[test]
    fn given_nonexistent_state_dir_when_resume_then_fails_appropriately() {
        // Given: A valid workspace and a --state-dir that doesn't exist on disk
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let state_dir = td.path().join("does-not-exist");

        // When: We run shipper resume with the missing state dir
        let mut cmd = shipper_cmd();
        cmd.arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("resume")
            // Then: It fails because no state can be found
            .assert()
            .failure()
            .stderr(contains("no existing state found"));
    }
}
