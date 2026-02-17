use std::fs;
use std::path::Path;

use assert_cmd::Command;
use chrono::Utc;
use predicates::str::contains;
use tempfile::tempdir;

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

#[test]
fn given_invalid_workspace_config_when_running_plan_then_validation_fails() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
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
        .stderr(contains("Configuration validation failed"));
}

#[test]
fn given_partial_readiness_and_parallel_config_when_running_plan_then_it_succeeds() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());
    write_file(
        &td.path().join(".shipper.toml"),
        r#"
[readiness]
method = "both"

[parallel]
enabled = true
"#,
    );

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("demo@0.1.0"));
}

#[test]
fn given_unknown_format_when_running_plan_then_cli_rejects_the_value() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--format")
        .arg("yaml")
        .arg("plan")
        .assert()
        .failure()
        .stderr(contains("possible values"))
        .stderr(contains("text"))
        .stderr(contains("json"));
}

#[test]
fn given_active_lock_when_clean_without_force_then_lock_is_preserved_and_command_fails() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let abs_state = td.path().join(".shipper");
    fs::create_dir_all(&abs_state).expect("mkdir");
    let lock_path = abs_state.join(shipper::lock::LOCK_FILE);
    let lock_info = shipper::lock::LockInfo {
        pid: 12345,
        hostname: "test-host".to_string(),
        acquired_at: Utc::now(),
        plan_id: Some("plan-123".to_string()),
    };
    fs::write(
        &lock_path,
        serde_json::to_string(&lock_info).expect("serialize"),
    )
    .expect("write lock");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("clean")
        .assert()
        .failure()
        .stderr(contains("cannot clean: active lock exists"));

    assert!(lock_path.exists(), "lock should remain without --force");
}

#[test]
fn given_active_lock_when_clean_with_force_then_lock_and_state_files_are_removed() {
    let td = tempdir().expect("tempdir");
    create_workspace(td.path());

    let abs_state = td.path().join(".shipper");
    fs::create_dir_all(&abs_state).expect("mkdir");

    let state_path = abs_state.join(shipper::state::STATE_FILE);
    let receipt_path = abs_state.join(shipper::state::RECEIPT_FILE);
    let events_path = abs_state.join(shipper::events::EVENTS_FILE);
    let lock_path = abs_state.join(shipper::lock::LOCK_FILE);

    fs::write(&state_path, "{}").expect("write state");
    fs::write(&receipt_path, "{}").expect("write receipt");
    fs::write(&events_path, "{}").expect("write events");

    let lock_info = shipper::lock::LockInfo {
        pid: 12345,
        hostname: "test-host".to_string(),
        acquired_at: Utc::now(),
        plan_id: Some("plan-123".to_string()),
    };
    fs::write(
        &lock_path,
        serde_json::to_string(&lock_info).expect("serialize"),
    )
    .expect("write lock");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--state-dir")
        .arg(".shipper")
        .arg("clean")
        .arg("--force")
        .assert()
        .success();

    assert!(!state_path.exists(), "state file should be removed");
    assert!(!receipt_path.exists(), "receipt file should be removed");
    assert!(!events_path.exists(), "events file should be removed");
    assert!(!lock_path.exists(), "lock file should be removed");
}
