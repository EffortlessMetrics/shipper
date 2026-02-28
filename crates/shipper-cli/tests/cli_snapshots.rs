use std::process::Command;

use insta::assert_snapshot;

/// Normalize version strings so snapshots don't break on every release.
fn redact_version(raw: &str) -> String {
    raw.replace(env!("CARGO_PKG_VERSION"), "[VERSION]")
}

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

// ── Help texts ───────────────────────────────────────────────────────

#[test]
fn help_text() {
    let output = shipper_cmd()
        .arg("--help")
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("help_text", redact_version(&stdout));
}

#[test]
fn plan_help() {
    let output = shipper_cmd()
        .args(["plan", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("plan_help", redact_version(&stdout));
}

#[test]
fn publish_help() {
    let output = shipper_cmd()
        .args(["publish", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("publish_help", redact_version(&stdout));
}

#[test]
fn resume_help() {
    let output = shipper_cmd()
        .args(["resume", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("resume_help", redact_version(&stdout));
}

#[test]
fn preflight_help() {
    let output = shipper_cmd()
        .args(["preflight", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("preflight_help", redact_version(&stdout));
}

#[test]
fn status_help() {
    let output = shipper_cmd()
        .args(["status", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("status_help", redact_version(&stdout));
}

#[test]
fn doctor_help() {
    let output = shipper_cmd()
        .args(["doctor", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("doctor_help", redact_version(&stdout));
}

#[test]
fn config_help() {
    let output = shipper_cmd()
        .args(["config", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("config_help", redact_version(&stdout));
}

#[test]
fn ci_help() {
    let output = shipper_cmd()
        .args(["ci", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("ci_help", redact_version(&stdout));
}

#[test]
fn clean_help() {
    let output = shipper_cmd()
        .args(["clean", "--help"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("clean_help", redact_version(&stdout));
}

// ── Version ──────────────────────────────────────────────────────────

#[test]
fn version_flag() {
    let output = shipper_cmd()
        .arg("--version")
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot!("version_flag", redact_version(&stdout));
}

// ── Error cases ──────────────────────────────────────────────────────

#[test]
fn no_subcommand_shows_error() {
    let output = shipper_cmd().output().expect("failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("no_subcommand_error", redact_version(&stderr));
}

#[test]
fn unknown_subcommand_shows_error() {
    let output = shipper_cmd()
        .arg("nonexistent")
        .output()
        .expect("failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("unknown_subcommand_error", redact_version(&stderr));
}

#[test]
fn completion_missing_shell_arg() {
    let output = shipper_cmd()
        .arg("completion")
        .output()
        .expect("failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot!("completion_missing_shell", redact_version(&stderr));
}
