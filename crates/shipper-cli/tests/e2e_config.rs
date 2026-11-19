use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
}

// ── config init ──────────────────────────────────────────────────────

#[test]
#[serial_test::serial]
fn config_init_creates_default_file() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    shipper_cmd()
        .current_dir(td.path())
        .args(["config", "init"])
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
#[serial_test::serial]
fn config_init_output_flag_writes_to_custom_path() {
    let td = tempdir().expect("tempdir");
    let custom = td.path().join("custom-dir").join("my-config.toml");

    // Creation requires an existing parent directory.
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
#[serial_test::serial]
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

#[test]
#[serial_test::serial]
fn config_init_concurrent_creators_have_exactly_one_winner() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");
    let start = Arc::new(Barrier::new(3));
    let creators: Vec<_> = (0..2)
        .map(|_| {
            let start = Arc::clone(&start);
            let output = config_path.clone();
            thread::spawn(move || {
                start.wait();
                std::process::Command::new(assert_cmd::cargo::cargo_bin!("shipper-cli"))
                    .args(["config", "init", "--output"])
                    .arg(output)
                    .output()
                    .expect("run competing initializer")
            })
        })
        .collect();
    start.wait();
    let outputs: Vec<_> = creators
        .into_iter()
        .map(|creator| creator.join().expect("join initializer"))
        .collect();
    assert_eq!(
        outputs
            .iter()
            .filter(|output| output.status.success())
            .count(),
        1,
        "only one process may create the file"
    );
    let refusal = outputs
        .iter()
        .find(|output| !output.status.success())
        .expect("loser");
    assert!(refusal.stdout.is_empty());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("existing paths are preserved"));
    shipper_cmd()
        .args(["config", "validate", "--path"])
        .arg(&config_path)
        .assert()
        .success();
}

#[test]
#[serial_test::serial]
fn config_init_creation_failure_does_not_claim_success() {
    let td = tempdir().expect("tempdir");
    let missing_parent = td.path().join("missing/config.toml");
    let output = shipper_cmd()
        .args(["config", "init", "--output"])
        .arg(&missing_parent)
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Failed to create new config file"));
    assert!(!missing_parent.exists());
    assert!(!missing_parent.parent().expect("parent").exists());
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn config_init_preserves_existing_and_dangling_symlinks() {
    let td = tempdir().expect("tempdir");
    for existing_target in [true, false] {
        let name = if existing_target {
            "existing"
        } else {
            "dangling"
        };
        let target = td.path().join(format!("{name}-target.toml"));
        let link = td.path().join(format!("{name}-link.toml"));
        let sentinel = b"# Reviewed target bytes must survive.\nstrict_ownership = true\n";
        if existing_target {
            fs::write(&target, sentinel).expect("write target");
        }
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");
        let output = shipper_cmd()
            .args(["config", "init", "--output"])
            .arg(&link)
            .assert()
            .failure()
            .get_output()
            .clone();
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("existing paths are preserved"));
        assert!(
            fs::symlink_metadata(&link)
                .expect("retained link")
                .is_symlink()
        );
        assert_eq!(fs::read_link(&link).expect("link target"), target);
        if existing_target {
            assert_eq!(fs::read(&target).expect("retained target"), sentinel);
        } else {
            assert!(!target.exists(), "dangling target must remain absent");
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn config_init_partial_write_failure_is_reported_without_success() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join("partial.toml");
    // Limit only this child process. Ignoring SIGXFSZ makes the second write
    // return a real filesystem error after creating a partial regular file.
    let output = Command::new("/bin/sh")
        .args([
            "-c",
            "set -eu; trap '' XFSZ; ulimit -f 1; exec \"$@\"",
            "config-write-failure",
        ])
        .arg(assert_cmd::cargo::cargo_bin!("shipper-cli"))
        .args(["config", "init", "--output"])
        .arg(&config_path)
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to finish new config file"),
        "{stderr}"
    );
    assert!(stderr.contains("may be incomplete"), "{stderr}");
    let size = fs::metadata(&config_path).expect("partial new file").len();
    assert!(
        size > 0 && size <= 1024,
        "write-size limit must cause a partial write: {size}"
    );
}

// ── config validate ──────────────────────────────────────────────────

#[test]
#[serial_test::serial]
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
#[serial_test::serial]
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
#[serial_test::serial]
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
#[serial_test::serial]
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
#[serial_test::serial]
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
