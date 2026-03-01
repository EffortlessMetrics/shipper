//! Integration tests for the plan→engine flow.
//!
//! Covers: plan structure from a simple workspace, dependency ordering,
//! circular dependency detection, package filtering, and plan_id determinism.

use std::fs;
use std::path::Path;

use tempfile::tempdir;

use shipper::plan;
use shipper::types::{Registry, ReleaseSpec};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}

fn spec_for(root: &Path) -> ReleaseSpec {
    ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    }
}

/// Two-crate workspace: `leaf` (no deps) and `top` (depends on `leaf`).
fn create_simple_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["leaf", "top"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("leaf/Cargo.toml"),
        r#"
[package]
name = "leaf"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("leaf/src/lib.rs"), "pub fn leaf() {}\n");

    write_file(
        &root.join("top/Cargo.toml"),
        r#"
[package]
name = "top"
version = "0.2.0"
edition = "2021"

[dependencies]
leaf = { path = "../leaf", version = "0.1.0" }
"#,
    );
    write_file(&root.join("top/src/lib.rs"), "pub fn top() {}\n");
}

/// Diamond workspace: `base` → `left`+`right` → `apex`.
fn create_diamond_workspace(root: &Path) {
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["base", "left", "right", "apex"]
resolver = "2"
"#,
    );

    write_file(
        &root.join("base/Cargo.toml"),
        r#"
[package]
name = "base"
version = "0.1.0"
edition = "2021"
"#,
    );
    write_file(&root.join("base/src/lib.rs"), "pub fn base() {}\n");

    write_file(
        &root.join("left/Cargo.toml"),
        r#"
[package]
name = "left"
version = "0.1.0"
edition = "2021"

[dependencies]
base = { path = "../base", version = "0.1.0" }
"#,
    );
    write_file(&root.join("left/src/lib.rs"), "pub fn left() {}\n");

    write_file(
        &root.join("right/Cargo.toml"),
        r#"
[package]
name = "right"
version = "0.1.0"
edition = "2021"

[dependencies]
base = { path = "../base", version = "0.1.0" }
"#,
    );
    write_file(&root.join("right/src/lib.rs"), "pub fn right() {}\n");

    write_file(
        &root.join("apex/Cargo.toml"),
        r#"
[package]
name = "apex"
version = "0.1.0"
edition = "2021"

[dependencies]
left = { path = "../left", version = "0.1.0" }
right = { path = "../right", version = "0.1.0" }
"#,
    );
    write_file(&root.join("apex/src/lib.rs"), "pub fn apex() {}\n");
}

// ===========================================================================
// 1. Build a plan from a simple workspace manifest, verify the plan structure
// ===========================================================================

#[test]
fn simple_workspace_plan_structure() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());

    let ws = plan::build_plan(&spec_for(td.path())).expect("build plan");

    assert_eq!(ws.plan.packages.len(), 2);
    assert_eq!(ws.plan.packages[0].name, "leaf");
    assert_eq!(ws.plan.packages[0].version, "0.1.0");
    assert_eq!(ws.plan.packages[1].name, "top");
    assert_eq!(ws.plan.packages[1].version, "0.2.0");

    // plan_id should be a 64-char hex string (SHA-256)
    assert_eq!(ws.plan.plan_id.len(), 64);
    assert!(ws.plan.plan_id.chars().all(|c| c.is_ascii_hexdigit()));

    // Registry should be crates-io
    assert_eq!(ws.plan.registry.name, "crates-io");

    // Dependencies map should reflect the graph
    let top_deps = ws.plan.dependencies.get("top").expect("top in deps map");
    assert!(top_deps.contains(&"leaf".to_string()));
    let leaf_deps = ws.plan.dependencies.get("leaf").expect("leaf in deps map");
    assert!(leaf_deps.is_empty());

    // No packages should be skipped
    assert!(ws.skipped.is_empty());
}

// ===========================================================================
// 2. Verify plan dependency ordering is correct (diamond graph)
// ===========================================================================

#[test]
fn diamond_dependency_ordering() {
    let td = tempdir().expect("tempdir");
    create_diamond_workspace(td.path());

    let ws = plan::build_plan(&spec_for(td.path())).expect("build plan");

    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 4);

    let pos = |name: &str| names.iter().position(|n| *n == name).unwrap();

    // `base` must come before `left` and `right`
    assert!(pos("base") < pos("left"));
    assert!(pos("base") < pos("right"));
    // `left` and `right` must come before `apex`
    assert!(pos("left") < pos("apex"));
    assert!(pos("right") < pos("apex"));

    // Deterministic ordering: `left` before `right` (alphabetical among peers)
    assert!(pos("left") < pos("right"));
}

// ===========================================================================
// 3. Verify plan handles circular dependencies gracefully
// ===========================================================================

#[test]
fn circular_dependency_is_detected_at_plan_level() {
    // Cargo itself rejects circular dependencies, so `cargo metadata` will
    // fail before our topo-sort runs. Verify the plan builder surfaces an
    // error rather than hanging or panicking.
    let td = tempdir().expect("tempdir");
    let root = td.path();

    // Create two crates that reference each other — cargo metadata will fail.
    write_file(
        &root.join("Cargo.toml"),
        r#"
[workspace]
members = ["x", "y"]
resolver = "2"
"#,
    );
    write_file(
        &root.join("x/Cargo.toml"),
        r#"
[package]
name = "x"
version = "0.1.0"
edition = "2021"

[dependencies]
y = { path = "../y", version = "0.1.0" }
"#,
    );
    write_file(&root.join("x/src/lib.rs"), "");
    write_file(
        &root.join("y/Cargo.toml"),
        r#"
[package]
name = "y"
version = "0.1.0"
edition = "2021"

[dependencies]
x = { path = "../x", version = "0.1.0" }
"#,
    );
    write_file(&root.join("y/src/lib.rs"), "");

    let err = plan::build_plan(&spec_for(root)).expect_err("cyclic deps should error");
    let msg = format!("{err:#}");
    // Cargo rejects cycles during metadata resolution
    assert!(
        msg.contains("cyclic")
            || msg.contains("cycle")
            || msg.contains("failed to execute cargo metadata"),
        "unexpected error message: {msg}"
    );
}

// ===========================================================================
// 4. Test plan filtering with --package flag equivalent
// ===========================================================================

#[test]
fn package_filter_selects_only_requested_and_transitive_deps() {
    let td = tempdir().expect("tempdir");
    create_diamond_workspace(td.path());

    // Select only `apex`: should pull in `left`, `right`, and `base` transitively.
    let mut spec = spec_for(td.path());
    spec.selected_packages = Some(vec!["apex".to_string()]);
    let ws = plan::build_plan(&spec).expect("build plan");

    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 4);
    assert!(names.contains(&"base"));
    assert!(names.contains(&"left"));
    assert!(names.contains(&"right"));
    assert!(names.contains(&"apex"));
}

#[test]
fn package_filter_leaf_only() {
    let td = tempdir().expect("tempdir");
    create_diamond_workspace(td.path());

    // Select only `left`: should pull in `base` (its dependency) but not `right` or `apex`.
    let mut spec = spec_for(td.path());
    spec.selected_packages = Some(vec!["left".to_string()]);
    let ws = plan::build_plan(&spec).expect("build plan");

    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["base", "left"]);
}

#[test]
fn package_filter_independent_crate_no_extra_deps() {
    let td = tempdir().expect("tempdir");
    create_diamond_workspace(td.path());

    // Select only `base`: no transitive deps needed.
    let mut spec = spec_for(td.path());
    spec.selected_packages = Some(vec!["base".to_string()]);
    let ws = plan::build_plan(&spec).expect("build plan");

    let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["base"]);
}

#[test]
fn package_filter_rejects_unknown_package() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());

    let mut spec = spec_for(td.path());
    spec.selected_packages = Some(vec!["nonexistent".to_string()]);
    let err = plan::build_plan(&spec).expect_err("should fail");
    assert!(format!("{err:#}").contains("selected package not found"));
}

// ===========================================================================
// 5. Verify plan_id generation is deterministic
// ===========================================================================

#[test]
fn plan_id_deterministic_across_calls() {
    let td = tempdir().expect("tempdir");
    create_diamond_workspace(td.path());

    let spec = spec_for(td.path());
    let ws1 = plan::build_plan(&spec).expect("plan 1");
    let ws2 = plan::build_plan(&spec).expect("plan 2");

    assert_eq!(ws1.plan.plan_id, ws2.plan.plan_id);
    // Packages and their order must also match
    assert_eq!(ws1.plan.packages.len(), ws2.plan.packages.len());
    for (a, b) in ws1.plan.packages.iter().zip(ws2.plan.packages.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.version, b.version);
    }
}

#[test]
fn plan_id_changes_with_different_registry() {
    let td = tempdir().expect("tempdir");
    create_simple_workspace(td.path());

    let spec_a = spec_for(td.path());
    let mut spec_b = spec_for(td.path());
    spec_b.registry = Registry {
        name: "my-registry".to_string(),
        api_base: "https://my-registry.example.com".to_string(),
        index_base: None,
    };

    let ws_a = plan::build_plan(&spec_a).expect("plan a");
    let ws_b = plan::build_plan(&spec_b).expect("plan b");

    assert_ne!(ws_a.plan.plan_id, ws_b.plan.plan_id);
}
