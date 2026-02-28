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

fn shipper_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("shipper"))
}

/// Create a simple workspace with a single crate.
fn create_simple_workspace(root: &Path) {
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

/// Create a workspace with multiple crates that have inter-dependencies.
fn create_multi_crate_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core-lib", "mid-lib", "top-app"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("core-lib/Cargo.toml"),
        r#"
[package]
name = "core-lib"
version = "0.2.0"
edition = "2021"
"#,
    );
    write_file(&root.join("core-lib/src/lib.rs"), "pub fn core() {}\n");

    write_file(
        &root.join("mid-lib/Cargo.toml"),
        r#"
[package]
name = "mid-lib"
version = "0.3.0"
edition = "2021"

[dependencies]
core-lib = { path = "../core-lib" }
"#,
    );
    write_file(
        &root.join("mid-lib/src/lib.rs"),
        "pub fn mid() { core_lib::core(); }\n",
    );

    write_file(
        &root.join("top-app/Cargo.toml"),
        r#"
[package]
name = "top-app"
version = "0.4.0"
edition = "2021"

[dependencies]
mid-lib = { path = "../mid-lib" }
"#,
    );
    write_file(
        &root.join("top-app/src/lib.rs"),
        "pub fn top() { mid_lib::mid(); }\n",
    );
}

#[test]
fn plan_simple_workspace() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0"))
        .stdout(contains("Total packages to publish: 1"));
}

#[test]
fn plan_package_filter() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("core-lib")
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("core-lib@0.2.0"))
        .stdout(contains("Total packages to publish: 1"));
}

#[test]
fn plan_non_workspace_directory_fails() {
    let td = tempdir().expect("tempdir");
    // Write a plain file, not a Cargo.toml
    write_file(&td.path().join("README.md"), "not a workspace");

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .failure();
}

#[test]
fn plan_explicit_manifest_path() {
    let td = tempdir().expect("tempdir");
    let nested = td.path().join("nested").join("project");
    create_simple_workspace(&nested);

    shipper_cmd()
        .arg("--manifest-path")
        .arg(nested.join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("alpha@0.1.0"));
}

#[test]
fn plan_output_is_deterministic() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let run = |cmd: &mut Command| -> String {
        let output = cmd
            .arg("--manifest-path")
            .arg(td.path().join("Cargo.toml"))
            .arg("plan")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(output).expect("utf8")
    };

    let first = run(&mut shipper_cmd());
    let second = run(&mut shipper_cmd());

    // Strip the plan_id line since that is a hash and should be stable,
    // but compare the rest to ensure order is identical.
    let strip_plan_id = |s: &str| -> Vec<String> {
        s.lines()
            .filter(|l| !l.starts_with("plan_id:") && !l.starts_with("workspace_root:"))
            .map(String::from)
            .collect::<Vec<_>>()
    };

    assert_eq!(strip_plan_id(&first), strip_plan_id(&second));
}

#[test]
fn plan_respects_dependency_ordering() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("utf8");

    // Extract package positions from numbered output lines like "  1. core-lib@0.2.0"
    let position_of = |name: &str| -> usize {
        stdout
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.contains(name) && trimmed.contains('.') {
                    // Parse the leading number: "1. core-lib@0.2.0"
                    trimmed.split('.').next().and_then(|n| n.trim().parse::<usize>().ok())
                } else {
                    None
                }
            })
            .next()
            .unwrap_or_else(|| panic!("package {name} not found in plan output:\n{stdout}"))
    };

    let core_pos = position_of("core-lib");
    let mid_pos = position_of("mid-lib");
    let top_pos = position_of("top-app");

    // core-lib has no deps, so it must come first.
    // mid-lib depends on core-lib, so it comes after.
    // top-app depends on mid-lib, so it comes last.
    assert!(
        core_pos < mid_pos,
        "core-lib (pos {core_pos}) should be before mid-lib (pos {mid_pos})"
    );
    assert!(
        mid_pos < top_pos,
        "mid-lib (pos {mid_pos}) should be before top-app (pos {top_pos})"
    );
}

#[test]
fn plan_package_filter_unknown_package() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());

    // Requesting a package that doesn't exist should produce an empty plan or error
    let output = shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("nonexistent-pkg")
        .arg("plan")
        .assert()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("utf8");
    // Should NOT contain the real package
    assert!(!stdout.contains("alpha@0.1.0"));
}

#[test]
fn plan_multi_package_filter() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("--package")
        .arg("core-lib")
        .arg("--package")
        .arg("mid-lib")
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("core-lib@0.2.0"))
        .stdout(contains("mid-lib@0.3.0"));
}

#[test]
fn plan_shows_all_workspace_members() {
    let td = tempdir().expect("tempdir");
    create_multi_crate_workspace(td.path());

    shipper_cmd()
        .arg("--manifest-path")
        .arg(td.path().join("Cargo.toml"))
        .arg("plan")
        .assert()
        .success()
        .stdout(contains("core-lib@0.2.0"))
        .stdout(contains("mid-lib@0.3.0"))
        .stdout(contains("top-app@0.4.0"))
        .stdout(contains("Total packages to publish: 3"));
}
