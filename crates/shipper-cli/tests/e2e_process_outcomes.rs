//! Spawned-process proof for the stable CLI outcome boundary.
//!
//! This first slice pins the process harness, terminal success, hard failure,
//! and the distinction between Clap's usage exit `2` and a Shipper partial
//! execution result. The deterministic partial-result fixtures follow after
//! the shared operator-outcome contract in #274.

use std::fs;
use std::path::Path;

use assert_cmd::Command;

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_no_execution_evidence(root: &Path) {
    let state_dir = root.join(".shipper");
    assert!(
        !state_dir.exists(),
        "process failed before execution but created misleading evidence at {}: {:?}",
        state_dir.display(),
        fs::read_dir(&state_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    );
}

#[test]
fn version_is_a_successful_terminal_process_result() {
    let root = tempfile::tempdir().expect("tempdir");
    let output = shipper_cmd()
        .current_dir(root.path())
        .arg("--version")
        .env("SHIPPER_PROCESS_TEST_SECRET", "process-secret-sentinel")
        .output()
        .expect("run shipper --version");

    assert_eq!(output.status.code(), Some(0), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(text.contains("shipper"), "{text}");
    assert!(!text.contains("process-secret-sentinel"), "{text}");
    assert_no_execution_evidence(root.path());
}

#[test]
fn missing_manifest_is_a_hard_failure_without_terminal_evidence() {
    let root = tempfile::tempdir().expect("tempdir");
    let missing_manifest = root.path().join("missing").join("Cargo.toml");
    let output = shipper_cmd()
        .current_dir(root.path())
        .arg("--manifest-path")
        .arg(&missing_manifest)
        .arg("plan")
        .env("SHIPPER_PROCESS_TEST_SECRET", "process-secret-sentinel")
        .output()
        .expect("run shipper plan with missing manifest");

    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(
        text.contains("manifest") || text.contains("Cargo.toml"),
        "hard failure should explain the unavailable manifest: {text}"
    );
    assert!(!text.contains("safe_to_rerun"), "{text}");
    assert!(!text.contains("process-secret-sentinel"), "{text}");
    assert_no_execution_evidence(root.path());
}

#[test]
fn clap_usage_exit_two_is_not_a_shipper_partial_result() {
    let root = tempfile::tempdir().expect("tempdir");
    let output = shipper_cmd()
        .current_dir(root.path())
        .arg("--definitely-not-a-shipper-option")
        .env("SHIPPER_PROCESS_TEST_SECRET", "process-secret-sentinel")
        .output()
        .expect("run shipper with invalid option");

    assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(
        text.contains("unexpected argument") || text.contains("Usage:"),
        "parser failure should remain recognizable as usage output: {text}"
    );
    assert!(!text.contains("partial_failure"), "{text}");
    assert!(!text.contains("safe_to_rerun"), "{text}");
    assert!(!text.contains("shipper resume"), "{text}");
    assert!(!text.contains("process-secret-sentinel"), "{text}");
    assert_no_execution_evidence(root.path());
}
