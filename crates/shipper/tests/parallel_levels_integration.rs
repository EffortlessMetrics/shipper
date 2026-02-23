use std::fs;
use std::path::Path;

use shipper::plan;
use shipper::types::{Registry, ReleaseSpec};
use tempfile::tempdir;

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

fn create_parallel_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["core", "api", "cli", "app"]
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
        &root.join("api/Cargo.toml"),
        r#"
[package]
name = "api"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core", version = "0.1.0" }
"#,
    );
    write_file(&root.join("api/src/lib.rs"), "pub fn api() {}\n");

    write_file(
        &root.join("cli/Cargo.toml"),
        r#"
[package]
name = "cli"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../core", version = "0.1.0" }
"#,
    );
    write_file(&root.join("cli/src/lib.rs"), "pub fn cli() {}\n");

    write_file(
        &root.join("app/Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
api = { path = "../api", version = "0.1.0" }
cli = { path = "../cli", version = "0.1.0" }
"#,
    );
    write_file(&root.join("app/src/lib.rs"), "pub fn app() {}\n");
}

#[test]
fn build_plan_groups_independent_packages_into_same_parallel_level() {
    let td = tempdir().expect("tempdir");
    create_parallel_workspace(td.path());

    let spec = ReleaseSpec {
        manifest_path: td.path().join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };
    let ws = plan::build_plan(&spec).expect("build plan");
    let levels = ws.plan.group_by_levels();

    assert_eq!(levels.len(), 3);
    assert_eq!(
        levels[0]
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["core"]
    );
    assert_eq!(
        levels[1]
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["api", "cli"]
    );
    assert_eq!(
        levels[2]
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["app"]
    );
}
