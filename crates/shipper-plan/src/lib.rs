//! # Plan
//!
//! Workspace analysis and deterministic publish-plan generation.
//!
//! This crate reads workspace metadata via `cargo_metadata`, filters
//! publishable crates, and produces a topologically-sorted
//! [`ReleasePlan`] that guarantees
//! dependencies are published before their dependents.
//!
//! ## Workflow
//!
//! 1. Load workspace metadata from the given `Cargo.toml`.
//! 2. Filter crates based on their `publish` field and the target registry.
//! 3. Optionally narrow the set to user-selected packages (plus transitive deps).
//! 4. Topologically sort the remaining crates and compute a stable plan ID.
//!
//! The resulting [`PlannedWorkspace`] is the input to preflight and publish
//! operations in the engine crate.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use cargo_metadata::{DependencyKind, Metadata, PackageId};
use chrono::Utc;
use sha2::{Digest, Sha256};
use shipper_types::{PlannedPackage, ReleasePlan, ReleaseSpec};

/// A workspace package that was excluded from the publish plan.
///
/// Packages are skipped when their `publish` field in `Cargo.toml`
/// is `false` or does not include the target registry.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct SkippedPackage {
    /// Crate name as declared in `Cargo.toml`.
    pub name: String,
    /// Crate version string.
    pub version: String,
    /// Human-readable reason the package was excluded.
    pub reason: String,
}

/// The output of [`build_plan`]: a publish plan plus context.
///
/// Contains the workspace root path, the deterministic [`ReleasePlan`],
/// and a list of packages that were skipped (with reasons).
#[derive(Debug, Clone)]
pub struct PlannedWorkspace {
    /// Absolute path to the workspace root directory.
    pub workspace_root: PathBuf,
    /// The deterministic, SHA256-identified publish plan.
    pub plan: ReleasePlan,
    /// Packages that were excluded from the plan.
    pub skipped: Vec<SkippedPackage>,
}

/// Build a deterministic publish plan from a [`ReleaseSpec`].
///
/// Reads the workspace via `cargo_metadata`, filters publishable crates
/// based on the target registry, topologically sorts them, and returns a
/// [`PlannedWorkspace`] ready for preflight or publish execution.
///
/// # Errors
///
/// Returns an error if:
/// - `cargo metadata` fails (e.g. invalid manifest path)
/// - A selected package is not found or not publishable
/// - A publishable crate depends on a non-publishable workspace member
/// - A dependency cycle is detected
pub fn build_plan(spec: &ReleaseSpec) -> Result<PlannedWorkspace> {
    let metadata = load_metadata(&spec.manifest_path)?;
    let workspace_root = metadata.workspace_root.clone().into_std_path_buf();

    let pkg_map = metadata
        .packages
        .iter()
        .map(|p| (p.id.clone(), p))
        .collect::<BTreeMap<PackageId, &cargo_metadata::Package>>();

    let workspace_ids: BTreeSet<PackageId> = metadata.workspace_members.iter().cloned().collect();

    // Track skipped packages (publish=false or not in registry list)
    let mut skipped: Vec<SkippedPackage> = Vec::new();

    // Workspace publishable set (restricted by `[package] publish` where possible).
    let publishable: BTreeSet<PackageId> = workspace_ids
        .iter()
        .filter_map(|id| {
            let pkg = pkg_map.get(id)?;
            if publish_allowed(pkg, &spec.registry.name) {
                Some(id.clone())
            } else {
                // Track why this package was skipped
                let reason = match &pkg.publish {
                    None => "publish not specified (default allowed)".to_string(),
                    Some(list) if list.is_empty() => "publish = false".to_string(),
                    Some(list) => format!("publish = {} (registry not in list)", list.join(", ")),
                };
                skipped.push(SkippedPackage {
                    name: pkg.name.to_string(),
                    version: pkg.version.to_string(),
                    reason,
                });
                None
            }
        })
        .collect();

    // Build dependency edges A->deps (restricted to publishable workspace members).
    let resolve = metadata
        .resolve
        .as_ref()
        .context("cargo metadata did not include a resolve graph")?;

    let mut deps_of: BTreeMap<PackageId, BTreeSet<PackageId>> = BTreeMap::new();
    let mut dependents_of: BTreeMap<PackageId, BTreeSet<PackageId>> = BTreeMap::new();

    for node in &resolve.nodes {
        if !publishable.contains(&node.id) {
            continue;
        }
        for dep in &node.deps {
            if !publishable.contains(&dep.pkg) {
                continue;
            }

            let is_relevant = dep
                .dep_kinds
                .iter()
                .any(|k| matches!(k.kind, DependencyKind::Normal | DependencyKind::Build));
            if !is_relevant {
                continue;
            }

            deps_of
                .entry(node.id.clone())
                .or_default()
                .insert(dep.pkg.clone());
            dependents_of
                .entry(dep.pkg.clone())
                .or_default()
                .insert(node.id.clone());
        }
    }

    // Determine which nodes to include.
    let included: BTreeSet<PackageId> = if let Some(sel) = &spec.selected_packages {
        // Map package name -> id (workspace publishable only).
        let mut name_to_id: BTreeMap<String, PackageId> = BTreeMap::new();
        for id in &publishable {
            let pkg = pkg_map
                .get(id)
                .context("workspace package missing from metadata")?;
            name_to_id.insert(pkg.name.to_string(), id.clone());
        }

        let mut queue: VecDeque<PackageId> = VecDeque::new();
        let mut set: BTreeSet<PackageId> = BTreeSet::new();

        for name in sel {
            let id = name_to_id
                .get(name)
                .with_context(|| format!("selected package not found or not publishable: {name}"))?
                .clone();
            if set.insert(id.clone()) {
                queue.push_back(id);
            }
        }

        // Include internal dependencies transitively.
        while let Some(id) = queue.pop_front() {
            if let Some(deps) = deps_of.get(&id) {
                for dep in deps {
                    if set.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        set
    } else {
        publishable.clone()
    };

    // Validate: included crates must not have normal/build deps on non-publishable workspace members.
    for node in &resolve.nodes {
        if !included.contains(&node.id) {
            continue;
        }
        for dep in &node.deps {
            // Skip deps that are publishable or not workspace members
            if publishable.contains(&dep.pkg) || !workspace_ids.contains(&dep.pkg) {
                continue;
            }
            let is_normal_or_build = dep
                .dep_kinds
                .iter()
                .any(|k| matches!(k.kind, DependencyKind::Normal | DependencyKind::Build));
            if is_normal_or_build {
                let pkg_name = pkg_map
                    .get(&node.id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("unknown");
                let dep_name = pkg_map
                    .get(&dep.pkg)
                    .map(|p| p.name.as_str())
                    .unwrap_or("unknown");
                bail!(
                    "publishable package '{}' depends on non-publishable workspace member '{}'",
                    pkg_name,
                    dep_name
                );
            }
        }
    }

    // Topological sort on included nodes.
    let order = topo_sort(&included, &deps_of, &dependents_of, &pkg_map)?;

    let packages: Vec<PlannedPackage> = order
        .iter()
        .map(|id| {
            let pkg = pkg_map.get(id).expect("pkg exists");
            PlannedPackage {
                name: pkg.name.to_string(),
                version: pkg.version.to_string(),
                manifest_path: pkg.manifest_path.clone().into_std_path_buf(),
            }
        })
        .collect();

    // Build dependency map for level-based parallel publishing
    let mut dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in &order {
        let pkg = pkg_map.get(id).expect("pkg exists");
        let pkg_name = pkg.name.to_string();

        // Get all dependencies of this package that are in the plan
        let dep_names: Vec<String> = deps_of
            .get(id)
            .map(|deps| {
                deps.iter()
                    .filter_map(|dep_id| {
                        if included.contains(dep_id) {
                            pkg_map.get(dep_id).map(|p| p.name.to_string())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        dependencies.insert(pkg_name, dep_names);
    }

    let plan_id = compute_plan_id(&spec.registry.api_base, &packages);

    Ok(PlannedWorkspace {
        workspace_root,
        plan: ReleasePlan {
            plan_version: shipper_state::CURRENT_PLAN_VERSION.to_string(),
            plan_id,
            created_at: Utc::now(),
            registry: spec.registry.clone(),
            packages,
            dependencies,
        },
        skipped,
    })
}

fn load_metadata(manifest_path: &Path) -> Result<Metadata> {
    shipper_cargo::load_metadata(manifest_path)
}

fn publish_allowed(pkg: &cargo_metadata::Package, registry_name: &str) -> bool {
    match &pkg.publish {
        None => true,
        Some(list) if list.is_empty() => false,
        Some(list) => {
            // Cargo uses `crates-io` as the default registry name.
            list.iter().any(|r| r == registry_name)
        }
    }
}

fn topo_sort(
    included: &BTreeSet<PackageId>,
    deps_of: &BTreeMap<PackageId, BTreeSet<PackageId>>,
    dependents_of: &BTreeMap<PackageId, BTreeSet<PackageId>>,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> Result<Vec<PackageId>> {
    let mut indegree: BTreeMap<PackageId, usize> = BTreeMap::new();
    for id in included {
        let deps = deps_of.get(id).cloned().unwrap_or_default();
        let count = deps.into_iter().filter(|d| included.contains(d)).count();
        indegree.insert(id.clone(), count);
    }

    // Deterministic queue: sort by package name.
    let mut ready: BTreeSet<(String, PackageId)> = BTreeSet::new();
    for (id, deg) in &indegree {
        if *deg == 0 {
            let name = pkg_map
                .get(id)
                .map(|p| p.name.to_string())
                .unwrap_or_else(|| String::from("unknown"));
            ready.insert((name, id.clone()));
        }
    }

    let mut out: Vec<PackageId> = Vec::with_capacity(included.len());

    while let Some((_, id)) = ready.iter().next().cloned() {
        ready.remove(&(pkg_map.get(&id).unwrap().name.to_string(), id.clone()));
        out.push(id.clone());

        if let Some(deps) = dependents_of.get(&id) {
            for dep in deps {
                if !included.contains(dep) {
                    continue;
                }
                let d = indegree
                    .get_mut(dep)
                    .expect("included package must have indegree");
                *d = d.saturating_sub(1);
                if *d == 0 {
                    let name = pkg_map.get(dep).unwrap().name.to_string();
                    ready.insert((name, dep.clone()));
                }
            }
        }
    }

    if out.len() != included.len() {
        bail!("dependency cycle detected within workspace publish set");
    }

    Ok(out)
}

fn compute_plan_id(registry_api_base: &str, packages: &[PlannedPackage]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(registry_api_base.as_bytes());
    hasher.update(b"\n");
    for p in packages {
        hasher.update(p.name.as_bytes());
        hasher.update(b"@");
        hasher.update(p.version.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use cargo_metadata::{MetadataCommand, PackageId};
    use proptest::prelude::*;
    use shipper_types::Registry;
    use tempfile::tempdir;

    use super::*;

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, content).expect("write");
    }

    fn create_workspace(root: &Path) {
        create_workspace_with_npdep(root, false);
    }

    fn create_workspace_with_npdep(root: &Path, include_npdep: bool) {
        let members = if include_npdep {
            r#"members = ["a", "b", "c", "d", "zeta", "alpha", "npdep"]"#
        } else {
            r#"members = ["a", "b", "c", "d", "zeta", "alpha"]"#
        };
        write_file(
            &root.join("Cargo.toml"),
            &format!(
                r#"
[workspace]
{members}
resolver = "2"
"#
            ),
        );

        write_file(
            &root.join("a/Cargo.toml"),
            r#"
[package]
name = "a"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("a/src/lib.rs"), "pub fn a() {}\n");

        write_file(
            &root.join("b/Cargo.toml"),
            r#"
[package]
name = "b"
version = "0.1.0"
edition = "2021"

[dependencies]
a = { path = "../a", version = "0.1.0" }
"#,
        );
        write_file(&root.join("b/src/lib.rs"), "pub fn b() {}\n");

        write_file(
            &root.join("c/Cargo.toml"),
            r#"
[package]
name = "c"
version = "0.1.0"
edition = "2021"
publish = false
"#,
        );
        write_file(&root.join("c/src/lib.rs"), "pub fn c() {}\n");

        write_file(
            &root.join("d/Cargo.toml"),
            r#"
[package]
name = "d"
version = "0.1.0"
edition = "2021"
publish = ["private-reg"]
"#,
        );
        write_file(&root.join("d/src/lib.rs"), "pub fn d() {}\n");

        write_file(
            &root.join("zeta/Cargo.toml"),
            r#"
[package]
name = "zeta"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("zeta/src/lib.rs"), "pub fn zeta() {}\n");

        write_file(
            &root.join("alpha/Cargo.toml"),
            r#"
[package]
name = "alpha"
version = "0.1.0"
edition = "2021"

[dev-dependencies]
a = { path = "../a", version = "0.1.0" }
"#,
        );
        write_file(&root.join("alpha/src/lib.rs"), "pub fn alpha() {}\n");

        if include_npdep {
            write_file(
                &root.join("npdep/Cargo.toml"),
                r#"
[package]
name = "npdep"
version = "0.1.0"
edition = "2021"

[dependencies]
c = { path = "../c", version = "0.1.0" }
"#,
            );
            write_file(&root.join("npdep/src/lib.rs"), "pub fn npdep() {}\n");
        }
    }

    fn spec_for(root: &Path) -> ReleaseSpec {
        ReleaseSpec {
            manifest_path: root.join("Cargo.toml"),
            registry: Registry::crates_io(),
            selected_packages: None,
        }
    }

    #[test]
    fn build_plan_filters_publishability_and_orders_dependencies() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<String> = ws.plan.packages.iter().map(|p| p.name.clone()).collect();

        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"zeta".to_string()));
        assert!(!names.contains(&"c".to_string()));
        assert!(!names.contains(&"d".to_string()));

        let a_idx = names.iter().position(|n| n == "a").expect("a present");
        let b_idx = names.iter().position(|n| n == "b").expect("b present");
        assert!(a_idx < b_idx);
    }

    #[test]
    fn build_plan_rejects_publishable_depending_on_non_publishable() {
        let td = tempdir().expect("tempdir");
        create_workspace_with_npdep(td.path(), true);

        // When npdep is included (all packages selected), the error should fire.
        let err = build_plan(&spec_for(td.path())).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(
                "publishable package 'npdep' depends on non-publishable workspace member 'c'"
            ),
            "unexpected error: {msg}"
        );

        // When only npdep is explicitly selected, the error should still fire.
        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["npdep".to_string()]);
        let err2 = build_plan(&spec).expect_err("must fail for selected npdep");
        let msg2 = format!("{err2:#}");
        assert!(
            msg2.contains(
                "publishable package 'npdep' depends on non-publishable workspace member 'c'"
            ),
            "unexpected error: {msg2}"
        );
    }

    #[test]
    fn build_plan_package_selection_ignores_unrelated_invalid_deps() {
        let td = tempdir().expect("tempdir");
        create_workspace_with_npdep(td.path(), true);

        // Selecting only "a" should succeed even though "npdep" (not selected)
        // depends on non-publishable "c".
        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["a".to_string()]);
        let ws = build_plan(&spec).expect("plan should succeed");
        let names: Vec<String> = ws.plan.packages.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, vec!["a".to_string()]);
    }

    #[test]
    fn build_plan_allows_dev_dep_on_non_publishable() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // alpha has a dev-dependency on a (which is publishable), but let's verify
        // that the plan succeeds — dev-deps on non-publishable crates are also fine.
        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert!(ws.plan.packages.iter().any(|p| p.name == "alpha"));
    }

    #[test]
    fn build_plan_selected_packages_include_internal_dependencies() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["b".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<String> = ws.plan.packages.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn build_plan_selected_single_package_does_not_include_dependents() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["a".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<String> = ws.plan.packages.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, vec!["a".to_string()]);
    }

    #[test]
    fn build_plan_errors_for_unknown_selected_package() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["does-not-exist".to_string()]);
        let err = build_plan(&spec).expect_err("must fail");
        assert!(format!("{err:#}").contains("selected package not found"));
    }

    #[test]
    fn topo_sort_reports_cycles() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let manifest = td.path().join("Cargo.toml");

        let metadata = MetadataCommand::new()
            .manifest_path(&manifest)
            .exec()
            .expect("metadata");

        let pkg_map = metadata
            .packages
            .iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<PackageId, &cargo_metadata::Package>>();
        let mut by_name = BTreeMap::<String, PackageId>::new();
        for pkg in &metadata.packages {
            by_name.insert(pkg.name.to_string(), pkg.id.clone());
        }

        let a = by_name.get("a").expect("a").clone();
        let b = by_name.get("b").expect("b").clone();

        let included = [a.clone(), b.clone()].into_iter().collect::<BTreeSet<_>>();
        let deps_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);
        let dependents_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);

        let err = topo_sort(&included, &deps_of, &dependents_of, &pkg_map).expect_err("cycle");
        assert!(format!("{err:#}").contains("dependency cycle detected"));
    }

    #[test]
    fn build_plan_is_deterministic_for_independent_nodes_by_name() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let alpha_idx = ws
            .plan
            .packages
            .iter()
            .position(|p| p.name == "alpha")
            .expect("alpha");
        let zeta_idx = ws
            .plan
            .packages
            .iter()
            .position(|p| p.name == "zeta")
            .expect("zeta");
        assert!(alpha_idx < zeta_idx);
    }

    #[test]
    fn build_plan_errors_for_missing_manifest() {
        let spec = ReleaseSpec {
            manifest_path: Path::new("missing").join("Cargo.toml"),
            registry: Registry::crates_io(),
            selected_packages: None,
        };
        let err = build_plan(&spec).expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to execute cargo metadata"));
    }

    // --- Single-crate workspace ---

    fn create_single_crate_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["only"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("only/Cargo.toml"),
            r#"
[package]
name = "only"
version = "1.2.3"
edition = "2021"
"#,
        );
        write_file(&root.join("only/src/lib.rs"), "pub fn only() {}\n");
    }

    #[test]
    fn build_plan_single_crate_workspace() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages.len(), 1);
        assert_eq!(ws.plan.packages[0].name, "only");
        assert_eq!(ws.plan.packages[0].version, "1.2.3");
        assert!(ws.skipped.is_empty());
        // Single crate has no internal deps
        assert_eq!(ws.plan.dependencies.get("only").map(|v| v.len()), Some(0));
    }

    // --- Determinism: same input produces identical plans ---

    #[test]
    fn build_plan_deterministic_across_runs() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let spec = spec_for(td.path());

        let ws1 = build_plan(&spec).expect("plan1");
        let ws2 = build_plan(&spec).expect("plan2");

        let names1: Vec<&str> = ws1.plan.packages.iter().map(|p| p.name.as_str()).collect();
        let names2: Vec<&str> = ws2.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names1, names2, "package order must be deterministic");
        assert_eq!(
            ws1.plan.plan_id, ws2.plan.plan_id,
            "plan_id must be deterministic"
        );
        assert_eq!(ws1.plan.dependencies, ws2.plan.dependencies);
    }

    // --- Skipped packages tracking ---

    #[test]
    fn build_plan_tracks_skipped_packages() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let skipped_names: Vec<&str> = ws.skipped.iter().map(|s| s.name.as_str()).collect();
        // c (publish = false) and d (publish = ["private-reg"]) should be skipped for crates-io
        assert!(
            skipped_names.contains(&"c"),
            "c should be skipped (publish=false)"
        );
        assert!(
            skipped_names.contains(&"d"),
            "d should be skipped (wrong registry)"
        );
        assert_eq!(ws.skipped.len(), 2);
    }

    // --- Private registry: d is included when targeting "private-reg" ---

    #[test]
    fn build_plan_includes_crate_when_registry_matches() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let spec = ReleaseSpec {
            manifest_path: td.path().join("Cargo.toml"),
            registry: Registry {
                name: "private-reg".to_string(),
                api_base: "https://private.example.com".to_string(),
                index_base: None,
            },
            selected_packages: None,
        };
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // d publishes to private-reg, so it should be included
        assert!(names.contains(&"d"));
        // c is publish=false, still excluded
        assert!(!names.contains(&"c"));
    }

    // --- Dependencies map correctness ---

    #[test]
    fn build_plan_dependencies_map_reflects_edges() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        // b depends on a
        let b_deps = ws.plan.dependencies.get("b").expect("b in deps map");
        assert!(b_deps.contains(&"a".to_string()));
        // a has no internal deps
        let a_deps = ws.plan.dependencies.get("a").expect("a in deps map");
        assert!(a_deps.is_empty());
        // alpha has dev-dep on a, which is NOT a normal dep so shouldn't appear
        let alpha_deps = ws
            .plan
            .dependencies
            .get("alpha")
            .expect("alpha in deps map");
        assert!(
            alpha_deps.is_empty(),
            "dev-deps should not appear in plan deps"
        );
    }

    // --- Plan version ---

    #[test]
    fn build_plan_sets_correct_plan_version() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.plan_version, shipper_state::CURRENT_PLAN_VERSION);
    }

    // --- publish_allowed unit tests ---

    #[test]
    fn publish_allowed_none_allows_all() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());
        let metadata = MetadataCommand::new()
            .manifest_path(td.path().join("Cargo.toml"))
            .exec()
            .expect("metadata");
        // "only" has no publish field (None) — should be allowed for any registry
        let pkg = metadata
            .packages
            .iter()
            .find(|p| p.name == "only")
            .expect("only");
        assert!(publish_allowed(pkg, "crates-io"));
        assert!(publish_allowed(pkg, "some-other-reg"));
    }

    #[test]
    fn publish_allowed_false_blocks_all() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let metadata = MetadataCommand::new()
            .manifest_path(td.path().join("Cargo.toml"))
            .exec()
            .expect("metadata");
        // "c" has publish = false → blocked everywhere
        let pkg = metadata.packages.iter().find(|p| p.name == "c").expect("c");
        assert!(!publish_allowed(pkg, "crates-io"));
        assert!(!publish_allowed(pkg, "private-reg"));
    }

    #[test]
    fn publish_allowed_list_matches_registry() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let metadata = MetadataCommand::new()
            .manifest_path(td.path().join("Cargo.toml"))
            .exec()
            .expect("metadata");
        // "d" has publish = ["private-reg"]
        let pkg = metadata.packages.iter().find(|p| p.name == "d").expect("d");
        assert!(publish_allowed(pkg, "private-reg"));
        assert!(!publish_allowed(pkg, "crates-io"));
    }

    // --- compute_plan_id changes when inputs differ ---

    #[test]
    fn compute_plan_id_differs_for_different_packages() {
        let pkgs_a = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("foo/Cargo.toml"),
        }];
        let pkgs_b = vec![PlannedPackage {
            name: "bar".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("bar/Cargo.toml"),
        }];
        let id_a = compute_plan_id("https://crates.io", &pkgs_a);
        let id_b = compute_plan_id("https://crates.io", &pkgs_b);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn compute_plan_id_differs_for_different_registries() {
        let pkgs = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("foo/Cargo.toml"),
        }];
        let id1 = compute_plan_id("https://crates.io", &pkgs);
        let id2 = compute_plan_id("https://private.example.com", &pkgs);
        assert_ne!(id1, id2);
    }

    #[test]
    fn compute_plan_id_differs_for_different_versions() {
        let pkgs1 = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("foo/Cargo.toml"),
        }];
        let pkgs2 = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "2.0.0".to_string(),
            manifest_path: PathBuf::from("foo/Cargo.toml"),
        }];
        let id1 = compute_plan_id("https://crates.io", &pkgs1);
        let id2 = compute_plan_id("https://crates.io", &pkgs2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn compute_plan_id_empty_packages() {
        let id = compute_plan_id("https://crates.io", &[]);
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // --- Workspace root is set correctly ---

    #[test]
    fn build_plan_sets_workspace_root() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        // The workspace_root should be a real path pointing at our temp dir
        assert!(ws.workspace_root.exists());
    }

    // --- topo_sort with no deps (all independent) produces name-sorted order ---

    #[test]
    fn topo_sort_independent_nodes_sorted_by_name() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let metadata = MetadataCommand::new()
            .manifest_path(td.path().join("Cargo.toml"))
            .exec()
            .expect("metadata");

        let pkg_map = metadata
            .packages
            .iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<PackageId, &cargo_metadata::Package>>();
        let mut by_name = BTreeMap::<String, PackageId>::new();
        for pkg in &metadata.packages {
            by_name.insert(pkg.name.to_string(), pkg.id.clone());
        }

        let alpha = by_name.get("alpha").expect("alpha").clone();
        let zeta = by_name.get("zeta").expect("zeta").clone();

        // Two independent nodes with no edges
        let included = [alpha.clone(), zeta.clone()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let deps_of = BTreeMap::new();
        let dependents_of = BTreeMap::new();

        let order = topo_sort(&included, &deps_of, &dependents_of, &pkg_map).expect("topo");
        let names: Vec<&str> = order
            .iter()
            .map(|id| pkg_map.get(id).unwrap().name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["alpha", "zeta"],
            "independent nodes sorted alphabetically"
        );
    }

    // --- Multi-crate deep chain ---

    #[test]
    fn build_plan_deep_dependency_chain() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["x", "y", "z"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("x/Cargo.toml"),
            r#"
[package]
name = "x"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&td.path().join("x/src/lib.rs"), "");
        write_file(
            &td.path().join("y/Cargo.toml"),
            r#"
[package]
name = "y"
version = "0.1.0"
edition = "2021"

[dependencies]
x = { path = "../x", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("y/src/lib.rs"), "");
        write_file(
            &td.path().join("z/Cargo.toml"),
            r#"
[package]
name = "z"
version = "0.1.0"
edition = "2021"

[dependencies]
y = { path = "../y", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("z/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["x", "y", "z"]);

        // Dependencies map: z->y, y->x, x->[]
        assert!(ws.plan.dependencies["x"].is_empty());
        assert_eq!(ws.plan.dependencies["y"], vec!["x".to_string()]);
        assert_eq!(ws.plan.dependencies["z"], vec!["y".to_string()]);
    }

    // --- All crates unpublishable produces empty plan ---

    #[test]
    fn build_plan_all_unpublishable_produces_empty_plan() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["priv"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("priv/Cargo.toml"),
            r#"
[package]
name = "priv"
version = "0.1.0"
edition = "2021"
publish = false
"#,
        );
        write_file(&td.path().join("priv/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert!(ws.plan.packages.is_empty());
        assert_eq!(ws.skipped.len(), 1);
        assert_eq!(ws.skipped[0].name, "priv");
    }

    // --- Selecting a non-publishable package errors ---

    #[test]
    fn build_plan_selecting_non_publishable_package_errors() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        // c is publish=false, not in the publishable set
        spec.selected_packages = Some(vec!["c".to_string()]);
        let err = build_plan(&spec).expect_err("must fail");
        assert!(format!("{err:#}").contains("selected package not found or not publishable"));
    }

    // --- Plan registry matches spec registry ---

    #[test]
    fn build_plan_registry_in_output_matches_spec() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.registry.name, "crates-io");
        assert_eq!(ws.plan.registry.api_base, "https://crates.io");
    }

    // ── Insta snapshot helpers ──────────────────────────────────────────

    /// Stable, redacted summary of a plan suitable for snapshot testing.
    /// Dynamic fields (plan_id, created_at, manifest_path, workspace_root) are
    /// replaced with deterministic placeholders so snapshots stay stable across
    /// machines and runs.
    #[derive(serde::Serialize)]
    struct PlanSnapshot {
        packages: Vec<PkgSnapshot>,
        dependencies: std::collections::BTreeMap<String, Vec<String>>,
        skipped: Vec<SkippedPackage>,
        registry_name: String,
    }

    #[derive(serde::Serialize)]
    struct PkgSnapshot {
        name: String,
        version: String,
    }

    fn snapshot_of(ws: &PlannedWorkspace) -> PlanSnapshot {
        PlanSnapshot {
            packages: ws
                .plan
                .packages
                .iter()
                .map(|p| PkgSnapshot {
                    name: p.name.clone(),
                    version: p.version.clone(),
                })
                .collect(),
            dependencies: ws.plan.dependencies.clone(),
            skipped: ws.skipped.clone(),
            registry_name: ws.plan.registry.name.clone(),
        }
    }

    // ── Insta snapshot tests ────────────────────────────────────────────

    #[test]
    fn snapshot_single_crate_plan() {
        let td = tempdir().expect("tempdir");
        create_single_crate_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("single_crate_plan", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_multi_crate_plan_with_deps() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("multi_crate_plan_with_deps", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_deep_chain_plan() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["x", "y", "z"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("x/Cargo.toml"),
            r#"
[package]
name = "x"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&td.path().join("x/src/lib.rs"), "");
        write_file(
            &td.path().join("y/Cargo.toml"),
            r#"
[package]
name = "y"
version = "0.1.0"
edition = "2021"

[dependencies]
x = { path = "../x", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("y/src/lib.rs"), "");
        write_file(
            &td.path().join("z/Cargo.toml"),
            r#"
[package]
name = "z"
version = "0.1.0"
edition = "2021"

[dependencies]
y = { path = "../y", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("z/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("deep_chain_plan", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_package_selection() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["b".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        insta::assert_yaml_snapshot!("package_selection_b", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_error_unknown_package() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["does-not-exist".to_string()]);
        let err = build_plan(&spec).expect_err("must fail");
        insta::assert_snapshot!("error_unknown_package", format!("{err:#}"));
    }

    #[test]
    fn snapshot_error_non_publishable_dep() {
        let td = tempdir().expect("tempdir");
        create_workspace_with_npdep(td.path(), true);

        let err = build_plan(&spec_for(td.path())).expect_err("must fail");
        insta::assert_snapshot!("error_non_publishable_dep", format!("{err:#}"));
    }

    #[test]
    fn snapshot_error_selecting_non_publishable() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["c".to_string()]);
        let err = build_plan(&spec).expect_err("must fail");
        insta::assert_snapshot!("error_selecting_non_publishable", format!("{err:#}"));
    }

    #[test]
    fn snapshot_error_cycle_detection() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let metadata = MetadataCommand::new()
            .manifest_path(td.path().join("Cargo.toml"))
            .exec()
            .expect("metadata");

        let pkg_map = metadata
            .packages
            .iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<PackageId, &cargo_metadata::Package>>();
        let mut by_name = BTreeMap::<String, PackageId>::new();
        for pkg in &metadata.packages {
            by_name.insert(pkg.name.to_string(), pkg.id.clone());
        }

        let a = by_name.get("a").expect("a").clone();
        let b = by_name.get("b").expect("b").clone();

        let included = [a.clone(), b.clone()].into_iter().collect::<BTreeSet<_>>();
        let deps_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);
        let dependents_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);

        let err = topo_sort(&included, &deps_of, &dependents_of, &pkg_map).expect_err("cycle");
        insta::assert_snapshot!("error_cycle_detection", format!("{err:#}"));
    }

    #[test]
    fn snapshot_plan_summary_display() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let mut summary = String::new();
        summary.push_str(&format!("Registry: {}\n", ws.plan.registry.name));
        summary.push_str(&format!(
            "Packages to publish ({}):\n",
            ws.plan.packages.len()
        ));
        for (i, pkg) in ws.plan.packages.iter().enumerate() {
            let deps = ws
                .plan
                .dependencies
                .get(&pkg.name)
                .cloned()
                .unwrap_or_default();
            if deps.is_empty() {
                summary.push_str(&format!("  {}. {} v{}\n", i + 1, pkg.name, pkg.version));
            } else {
                summary.push_str(&format!(
                    "  {}. {} v{} (depends on: {})\n",
                    i + 1,
                    pkg.name,
                    pkg.version,
                    deps.join(", ")
                ));
            }
        }
        if !ws.skipped.is_empty() {
            summary.push_str(&format!("Skipped ({}):\n", ws.skipped.len()));
            for s in &ws.skipped {
                summary.push_str(&format!("  - {} v{}: {}\n", s.name, s.version, s.reason));
            }
        }
        insta::assert_snapshot!("plan_summary_display", summary);
    }

    #[test]
    fn snapshot_skipped_packages_detail() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("skipped_packages_detail", &ws.skipped);
    }

    // ── Empty workspace (all packages unpublishable) ──────────────────

    #[test]
    fn build_plan_empty_workspace_all_unpublishable() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["internal-a", "internal-b"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("internal-a/Cargo.toml"),
            r#"
[package]
name = "internal-a"
version = "0.1.0"
edition = "2021"
publish = false
"#,
        );
        write_file(&td.path().join("internal-a/src/lib.rs"), "");
        write_file(
            &td.path().join("internal-b/Cargo.toml"),
            r#"
[package]
name = "internal-b"
version = "0.1.0"
edition = "2021"
publish = false
"#,
        );
        write_file(&td.path().join("internal-b/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert!(ws.plan.packages.is_empty());
        assert_eq!(ws.skipped.len(), 2);
        assert!(ws.plan.dependencies.is_empty());
    }

    // ── Linear dependency chain: A → B → C → D ────────────────────

    fn create_linear_chain_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["chain-a", "chain-b", "chain-c", "chain-d"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("chain-d/Cargo.toml"),
            r#"
[package]
name = "chain-d"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("chain-d/src/lib.rs"), "");
        write_file(
            &root.join("chain-c/Cargo.toml"),
            r#"
[package]
name = "chain-c"
version = "0.1.0"
edition = "2021"

[dependencies]
chain-d = { path = "../chain-d", version = "0.1.0" }
"#,
        );
        write_file(&root.join("chain-c/src/lib.rs"), "");
        write_file(
            &root.join("chain-b/Cargo.toml"),
            r#"
[package]
name = "chain-b"
version = "0.1.0"
edition = "2021"

[dependencies]
chain-c = { path = "../chain-c", version = "0.1.0" }
"#,
        );
        write_file(&root.join("chain-b/src/lib.rs"), "");
        write_file(
            &root.join("chain-a/Cargo.toml"),
            r#"
[package]
name = "chain-a"
version = "0.1.0"
edition = "2021"

[dependencies]
chain-b = { path = "../chain-b", version = "0.1.0" }
"#,
        );
        write_file(&root.join("chain-a/src/lib.rs"), "");
    }

    #[test]
    fn build_plan_linear_chain_a_b_c_d() {
        let td = tempdir().expect("tempdir");
        create_linear_chain_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["chain-d", "chain-c", "chain-b", "chain-a"]);

        // Verify dependency map
        assert!(ws.plan.dependencies["chain-d"].is_empty());
        assert_eq!(ws.plan.dependencies["chain-c"], vec!["chain-d".to_string()]);
        assert_eq!(ws.plan.dependencies["chain-b"], vec!["chain-c".to_string()]);
        assert_eq!(ws.plan.dependencies["chain-a"], vec!["chain-b".to_string()]);
    }

    #[test]
    fn build_plan_linear_chain_selecting_middle_pulls_transitive_deps() {
        let td = tempdir().expect("tempdir");
        create_linear_chain_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["chain-b".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // chain-b depends on chain-c which depends on chain-d
        assert_eq!(names, vec!["chain-d", "chain-c", "chain-b"]);
    }

    // ── Diamond dependency: A → B, A → C, B → D, C → D ────────────

    fn create_diamond_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["diamond-a", "diamond-b", "diamond-c", "diamond-d"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("diamond-d/Cargo.toml"),
            r#"
[package]
name = "diamond-d"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("diamond-d/src/lib.rs"), "");
        write_file(
            &root.join("diamond-b/Cargo.toml"),
            r#"
[package]
name = "diamond-b"
version = "0.1.0"
edition = "2021"

[dependencies]
diamond-d = { path = "../diamond-d", version = "0.1.0" }
"#,
        );
        write_file(&root.join("diamond-b/src/lib.rs"), "");
        write_file(
            &root.join("diamond-c/Cargo.toml"),
            r#"
[package]
name = "diamond-c"
version = "0.1.0"
edition = "2021"

[dependencies]
diamond-d = { path = "../diamond-d", version = "0.1.0" }
"#,
        );
        write_file(&root.join("diamond-c/src/lib.rs"), "");
        write_file(
            &root.join("diamond-a/Cargo.toml"),
            r#"
[package]
name = "diamond-a"
version = "0.1.0"
edition = "2021"

[dependencies]
diamond-b = { path = "../diamond-b", version = "0.1.0" }
diamond-c = { path = "../diamond-c", version = "0.1.0" }
"#,
        );
        write_file(&root.join("diamond-a/src/lib.rs"), "");
    }

    #[test]
    fn build_plan_diamond_dependency() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();

        // D must come first (no deps), then B and C (alphabetical, both depend on D), then A
        assert_eq!(
            names,
            vec!["diamond-d", "diamond-b", "diamond-c", "diamond-a"]
        );

        // Verify dependency edges
        assert!(ws.plan.dependencies["diamond-d"].is_empty());
        assert_eq!(
            ws.plan.dependencies["diamond-b"],
            vec!["diamond-d".to_string()]
        );
        assert_eq!(
            ws.plan.dependencies["diamond-c"],
            vec!["diamond-d".to_string()]
        );
        let mut a_deps = ws.plan.dependencies["diamond-a"].clone();
        a_deps.sort();
        assert_eq!(
            a_deps,
            vec!["diamond-b".to_string(), "diamond-c".to_string()]
        );
    }

    #[test]
    fn build_plan_diamond_all_deps_before_dependents() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();

        // D before B, D before C, B before A, C before A
        assert!(pos("diamond-d") < pos("diamond-b"));
        assert!(pos("diamond-d") < pos("diamond-c"));
        assert!(pos("diamond-b") < pos("diamond-a"));
        assert!(pos("diamond-c") < pos("diamond-a"));
    }

    // ── Wide flat workspace: 20 packages with no dependencies ──────

    fn create_wide_flat_workspace(root: &Path, count: usize) {
        let members: Vec<String> = (0..count).map(|i| format!("\"pkg-{i:02}\"")).collect();
        write_file(
            &root.join("Cargo.toml"),
            &format!(
                r#"
[workspace]
members = [{members}]
resolver = "2"
"#,
                members = members.join(", ")
            ),
        );
        for i in 0..count {
            let name = format!("pkg-{i:02}");
            write_file(
                &root.join(format!("{name}/Cargo.toml")),
                &format!(
                    r#"
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
"#
                ),
            );
            write_file(&root.join(format!("{name}/src/lib.rs")), "");
        }
    }

    #[test]
    fn build_plan_wide_flat_workspace_20_packages() {
        let td = tempdir().expect("tempdir");
        create_wide_flat_workspace(td.path(), 20);

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages.len(), 20);
        assert!(ws.skipped.is_empty());

        // All packages should have empty dependency lists
        for pkg in &ws.plan.packages {
            let deps = ws.plan.dependencies.get(&pkg.name).expect("in deps map");
            assert!(deps.is_empty(), "{} should have no deps", pkg.name);
        }

        // Independent packages are sorted alphabetically
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(names, sorted_names, "independent packages sorted by name");
    }

    // ── Package names with special characters (hyphens, underscores) ──

    fn create_special_names_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["my-hyphen-pkg", "my_underscore_pkg", "a-b_c-d_e"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("my-hyphen-pkg/Cargo.toml"),
            r#"
[package]
name = "my-hyphen-pkg"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("my-hyphen-pkg/src/lib.rs"), "");
        write_file(
            &root.join("my_underscore_pkg/Cargo.toml"),
            r#"
[package]
name = "my_underscore_pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
my-hyphen-pkg = { path = "../my-hyphen-pkg", version = "0.1.0" }
"#,
        );
        write_file(&root.join("my_underscore_pkg/src/lib.rs"), "");
        write_file(
            &root.join("a-b_c-d_e/Cargo.toml"),
            r#"
[package]
name = "a-b_c-d_e"
version = "0.2.0"
edition = "2021"
"#,
        );
        write_file(&root.join("a-b_c-d_e/src/lib.rs"), "");
    }

    #[test]
    fn build_plan_special_character_names() {
        let td = tempdir().expect("tempdir");
        create_special_names_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();

        assert!(names.contains(&"my-hyphen-pkg"));
        assert!(names.contains(&"my_underscore_pkg"));
        assert!(names.contains(&"a-b_c-d_e"));
        assert_eq!(ws.plan.packages.len(), 3);

        // my_underscore_pkg depends on my-hyphen-pkg, so hyphen comes first
        let hyphen_idx = names.iter().position(|n| *n == "my-hyphen-pkg").unwrap();
        let underscore_idx = names
            .iter()
            .position(|n| *n == "my_underscore_pkg")
            .unwrap();
        assert!(hyphen_idx < underscore_idx);
    }

    #[test]
    fn build_plan_special_names_dependency_map() {
        let td = tempdir().expect("tempdir");
        create_special_names_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert!(ws.plan.dependencies["my-hyphen-pkg"].is_empty());
        assert_eq!(
            ws.plan.dependencies["my_underscore_pkg"],
            vec!["my-hyphen-pkg".to_string()]
        );
        assert!(ws.plan.dependencies["a-b_c-d_e"].is_empty());
    }

    // ── Snapshot tests for various plan topologies ──────────────────

    #[test]
    fn snapshot_linear_chain_plan() {
        let td = tempdir().expect("tempdir");
        create_linear_chain_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("linear_chain_plan", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_diamond_plan() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("diamond_plan", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_wide_flat_plan() {
        let td = tempdir().expect("tempdir");
        create_wide_flat_workspace(td.path(), 5);

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("wide_flat_plan_5", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_special_names_plan() {
        let td = tempdir().expect("tempdir");
        create_special_names_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("special_names_plan", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_empty_workspace_plan() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["priv-a", "priv-b"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("priv-a/Cargo.toml"),
            r#"
[package]
name = "priv-a"
version = "0.1.0"
edition = "2021"
publish = false
"#,
        );
        write_file(&td.path().join("priv-a/src/lib.rs"), "");
        write_file(
            &td.path().join("priv-b/Cargo.toml"),
            r#"
[package]
name = "priv-b"
version = "0.1.0"
edition = "2021"
publish = false
"#,
        );
        write_file(&td.path().join("priv-b/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("empty_workspace_plan", snapshot_of(&ws));
    }

    // ── Plan stability: same input always produces same order ───────

    #[test]
    fn plan_stability_diamond_10_runs() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());
        let spec = spec_for(td.path());

        let baseline = build_plan(&spec).expect("plan");
        let baseline_names: Vec<&str> = baseline
            .plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        let baseline_id = &baseline.plan.plan_id;

        for _ in 0..10 {
            let ws = build_plan(&spec).expect("plan");
            let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, baseline_names, "order must be stable across runs");
            assert_eq!(&ws.plan.plan_id, baseline_id, "plan_id must be stable");
        }
    }

    #[test]
    fn plan_stability_linear_chain_10_runs() {
        let td = tempdir().expect("tempdir");
        create_linear_chain_workspace(td.path());
        let spec = spec_for(td.path());

        let baseline = build_plan(&spec).expect("plan");
        let baseline_names: Vec<&str> = baseline
            .plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect();

        for _ in 0..10 {
            let ws = build_plan(&spec).expect("plan");
            let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, baseline_names);
        }
    }

    #[test]
    fn plan_stability_wide_flat_10_runs() {
        let td = tempdir().expect("tempdir");
        create_wide_flat_workspace(td.path(), 10);
        let spec = spec_for(td.path());

        let baseline = build_plan(&spec).expect("plan");
        let baseline_names: Vec<&str> = baseline
            .plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect();

        for _ in 0..10 {
            let ws = build_plan(&spec).expect("plan");
            let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, baseline_names);
        }
    }

    // ── Build-dependency ordering ─────────────────────────────────────

    fn create_build_dep_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["codegen", "app"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("codegen/Cargo.toml"),
            r#"
[package]
name = "codegen"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("codegen/src/lib.rs"), "");
        write_file(
            &root.join("app/Cargo.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[build-dependencies]
codegen = { path = "../codegen", version = "0.1.0" }
"#,
        );
        write_file(&root.join("app/src/lib.rs"), "");
        write_file(&root.join("app/build.rs"), "fn main() {}");
    }

    #[test]
    fn build_plan_build_dependency_ordering() {
        let td = tempdir().expect("tempdir");
        create_build_dep_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // Build-dep codegen must appear before app
        assert_eq!(names, vec!["codegen", "app"]);
        assert_eq!(ws.plan.dependencies["app"], vec!["codegen".to_string()]);
    }

    #[test]
    fn snapshot_build_dep_plan() {
        let td = tempdir().expect("tempdir");
        create_build_dep_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("build_dep_plan", snapshot_of(&ws));
    }

    // ── Multiple package selection ────────────────────────────────────

    #[test]
    fn build_plan_multiple_selected_packages() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        // Select both "b" (depends on "a") and "zeta" (independent)
        spec.selected_packages = Some(vec!["b".to_string(), "zeta".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // "a" is pulled in transitively by "b"
        assert_eq!(names, vec!["a", "b", "zeta"]);
    }

    #[test]
    fn snapshot_multi_select_plan() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["b".to_string(), "zeta".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        insta::assert_yaml_snapshot!("multi_select_plan", snapshot_of(&ws));
    }

    // ── Selecting leaf is standalone ──────────────────────────────────

    #[test]
    fn build_plan_selecting_leaf_is_standalone() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        // diamond-d is the leaf (no deps); selecting it gives just that one
        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["diamond-d".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["diamond-d"]);
    }

    // ── Selecting all packages equals no selection ────────────────────

    #[test]
    fn build_plan_selecting_all_equals_no_selection() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws_all = build_plan(&spec_for(td.path())).expect("plan");
        let all_names: Vec<&str> = ws_all
            .plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect();

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(all_names.iter().map(|n| n.to_string()).collect());
        let ws_explicit = build_plan(&spec).expect("plan");
        let explicit_names: Vec<&str> = ws_explicit
            .plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect();

        assert_eq!(all_names, explicit_names);
        assert_eq!(ws_all.plan.plan_id, ws_explicit.plan.plan_id);
    }

    // ── Three-node cycle detection ───────────────────────────────────

    #[test]
    fn topo_sort_three_node_cycle() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let metadata = MetadataCommand::new()
            .manifest_path(td.path().join("Cargo.toml"))
            .exec()
            .expect("metadata");

        let pkg_map = metadata
            .packages
            .iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<PackageId, &cargo_metadata::Package>>();
        let mut by_name = BTreeMap::<String, PackageId>::new();
        for pkg in &metadata.packages {
            by_name.insert(pkg.name.to_string(), pkg.id.clone());
        }

        let a = by_name.get("a").expect("a").clone();
        let b = by_name.get("b").expect("b").clone();
        let alpha = by_name.get("alpha").expect("alpha").clone();

        // Synthetic cycle: a -> b -> alpha -> a
        let included = [a.clone(), b.clone(), alpha.clone()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let deps_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (
                b.clone(),
                [alpha.clone()].into_iter().collect::<BTreeSet<_>>(),
            ),
            (
                alpha.clone(),
                [a.clone()].into_iter().collect::<BTreeSet<_>>(),
            ),
        ]);
        let dependents_of = BTreeMap::from([
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
            (
                alpha.clone(),
                [b.clone()].into_iter().collect::<BTreeSet<_>>(),
            ),
            (
                a.clone(),
                [alpha.clone()].into_iter().collect::<BTreeSet<_>>(),
            ),
        ]);

        let err = topo_sort(&included, &deps_of, &dependents_of, &pkg_map).expect_err("cycle");
        assert!(format!("{err:#}").contains("dependency cycle detected"));
    }

    // ── Mixed versions ───────────────────────────────────────────────

    #[test]
    fn build_plan_mixed_versions() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["core", "util"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("core/Cargo.toml"),
            r#"
[package]
name = "core"
version = "2.5.0"
edition = "2021"
"#,
        );
        write_file(&td.path().join("core/src/lib.rs"), "");
        write_file(
            &td.path().join("util/Cargo.toml"),
            r#"
[package]
name = "util"
version = "0.3.1"
edition = "2021"

[dependencies]
core = { path = "../core", version = "2.5.0" }
"#,
        );
        write_file(&td.path().join("util/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages[0].name, "core");
        assert_eq!(ws.plan.packages[0].version, "2.5.0");
        assert_eq!(ws.plan.packages[1].name, "util");
        assert_eq!(ws.plan.packages[1].version, "0.3.1");
    }

    // ── Plan ID differs for different selections ─────────────────────

    #[test]
    fn build_plan_plan_id_differs_for_different_selections() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws_all = build_plan(&spec_for(td.path())).expect("plan");

        let mut spec_a = spec_for(td.path());
        spec_a.selected_packages = Some(vec!["a".to_string()]);
        let ws_a = build_plan(&spec_a).expect("plan");

        assert_ne!(ws_all.plan.plan_id, ws_a.plan.plan_id);
    }

    // ── Dev-deps excluded from transitive closure ────────────────────

    #[test]
    fn build_plan_dev_deps_excluded_from_transitive() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        // "alpha" has a dev-dep on "a"; selecting "alpha" should NOT pull in "a"
        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["alpha".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha"]);
    }

    // ── compute_plan_id boundary: name@version separator ─────────────

    #[test]
    fn compute_plan_id_no_collision_on_name_version_boundary() {
        // Ensure "foo@1.0.0" and "fo@o1.0.0" produce different IDs
        let pkgs_a = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("a/Cargo.toml"),
        }];
        let pkgs_b = vec![PlannedPackage {
            name: "fo".to_string(),
            version: "o1.0.0".to_string(),
            manifest_path: PathBuf::from("b/Cargo.toml"),
        }];
        let id_a = compute_plan_id("https://crates.io", &pkgs_a);
        let id_b = compute_plan_id("https://crates.io", &pkgs_b);
        assert_ne!(id_a, id_b);
    }

    // ── compute_plan_id is order-sensitive ────────────────────────────

    #[test]
    fn compute_plan_id_is_order_sensitive() {
        let pkg_a = PlannedPackage {
            name: "aaa".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("a/Cargo.toml"),
        };
        let pkg_b = PlannedPackage {
            name: "bbb".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("b/Cargo.toml"),
        };
        let id_ab = compute_plan_id("https://crates.io", &[pkg_a.clone(), pkg_b.clone()]);
        let id_ba = compute_plan_id("https://crates.io", &[pkg_b, pkg_a]);
        assert_ne!(id_ab, id_ba);
    }

    // ── compute_plan_id is valid SHA256 hex ──────────────────────────

    #[test]
    fn compute_plan_id_is_sha256_hex() {
        let pkgs = vec![
            PlannedPackage {
                name: "x".to_string(),
                version: "0.0.1".to_string(),
                manifest_path: PathBuf::from("x/Cargo.toml"),
            },
            PlannedPackage {
                name: "y".to_string(),
                version: "0.0.2".to_string(),
                manifest_path: PathBuf::from("y/Cargo.toml"),
            },
        ];
        let id = compute_plan_id("https://example.com", &pkgs);
        assert_eq!(id.len(), 64, "SHA256 hex digest must be 64 chars");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "all chars must be hex digits"
        );
    }

    // ── Dependencies map keys match planned packages exactly ─────────

    #[test]
    fn build_plan_deps_map_keys_match_packages() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let pkg_names: BTreeSet<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        let dep_keys: BTreeSet<&str> = ws.plan.dependencies.keys().map(|k| k.as_str()).collect();
        assert_eq!(pkg_names, dep_keys);
    }

    // ── Plan stability for build-dep workspace ───────────────────────

    #[test]
    fn plan_stability_build_dep_10_runs() {
        let td = tempdir().expect("tempdir");
        create_build_dep_workspace(td.path());
        let spec = spec_for(td.path());

        let baseline = build_plan(&spec).expect("plan");
        let baseline_names: Vec<&str> = baseline
            .plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect();

        for _ in 0..10 {
            let ws = build_plan(&spec).expect("plan");
            let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, baseline_names);
            assert_eq!(ws.plan.plan_id, baseline.plan.plan_id);
        }
    }

    proptest! {
        #[test]
        fn compute_plan_id_is_stable_and_hex(
            registry in "[a-z]{1,8}",
            packages in prop::collection::vec(("[a-z]{1,6}", 0u8..10u8, 0u8..10u8, 0u8..10u8), 1..8),
        ) {
            let pkgs: Vec<PlannedPackage> = packages
                .iter()
                .map(|(name, major, minor, patch)| PlannedPackage {
                    name: name.clone(),
                    version: format!("{}.{}.{}", major, minor, patch),
                    manifest_path: Path::new("x").join(format!("{name}.toml")),
                })
                .collect();

            let id1 = compute_plan_id(&registry, &pkgs);
            let id2 = compute_plan_id(&registry, &pkgs);
            prop_assert_eq!(&id1, &id2);
            prop_assert_eq!(id1.len(), 64);
            prop_assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
        }

        /// Property: plan_id is deterministic — same registry + packages = same id.
        #[test]
        fn prop_plan_id_deterministic_for_same_input(
            registry in "[a-z]{1,10}",
            pkg_count in 0usize..10,
        ) {
            let pkgs: Vec<PlannedPackage> = (0..pkg_count)
                .map(|i| PlannedPackage {
                    name: format!("crate-{i}"),
                    version: format!("{i}.0.0"),
                    manifest_path: Path::new("x").join(format!("crate-{i}.toml")),
                })
                .collect();

            let id1 = compute_plan_id(&registry, &pkgs);
            let id2 = compute_plan_id(&registry, &pkgs);
            prop_assert_eq!(id1, id2);
        }

        /// Property: plan ordering respects all dependencies (for linear chains).
        /// Generates chains of length 1..6 and verifies topo order.
        #[test]
        fn prop_plan_ordering_respects_dependencies(chain_len in 1usize..7) {
            // Build a linear chain workspace on disk and verify ordering
            let td = tempdir().expect("tempdir");
            let members: Vec<String> = (0..chain_len).map(|i| format!("\"p{i}\"")).collect();
            write_file(
                &td.path().join("Cargo.toml"),
                &format!(
                    "[workspace]\nmembers = [{members}]\nresolver = \"2\"\n",
                    members = members.join(", ")
                ),
            );

            for i in 0..chain_len {
                let name = format!("p{i}");
                let deps = if i > 0 {
                    let prev = format!("p{}", i - 1);
                    format!(
                        "\n[dependencies]\n{prev} = {{ path = \"../{prev}\", version = \"0.1.0\" }}\n"
                    )
                } else {
                    String::new()
                };
                write_file(
                    &td.path().join(format!("{name}/Cargo.toml")),
                    &format!(
                        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{deps}"
                    ),
                );
                write_file(&td.path().join(format!("{name}/src/lib.rs")), "");
            }

            let ws = build_plan(&spec_for(td.path())).expect("plan");
            let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();

            // Verify: for every package, all its deps appear earlier in the plan
            for pkg in &ws.plan.packages {
                let pkg_pos = names.iter().position(|n| *n == pkg.name).unwrap();
                if let Some(deps) = ws.plan.dependencies.get(&pkg.name) {
                    for dep in deps {
                        let dep_pos = names.iter().position(|n| n == dep).unwrap();
                        prop_assert!(
                            dep_pos < pkg_pos,
                            "dependency {dep} (pos {dep_pos}) must come before {name} (pos {pkg_pos})",
                            name = pkg.name
                        );
                    }
                }
            }
        }

        /// Property: different package lists produce different plan IDs (high probability).
        #[test]
        fn prop_plan_id_differs_for_distinct_packages(
            name_a in "[a-z]{1,6}",
            name_b in "[a-z]{1,6}",
            ver_a in 0u8..20u8,
            ver_b in 0u8..20u8,
        ) {
            // Only test when inputs actually differ
            prop_assume!(name_a != name_b || ver_a != ver_b);
            let pkgs_a = vec![PlannedPackage {
                name: name_a,
                version: format!("{ver_a}.0.0"),
                manifest_path: Path::new("a").join("Cargo.toml"),
            }];
            let pkgs_b = vec![PlannedPackage {
                name: name_b,
                version: format!("{ver_b}.0.0"),
                manifest_path: Path::new("b").join("Cargo.toml"),
            }];
            let id_a = compute_plan_id("https://crates.io", &pkgs_a);
            let id_b = compute_plan_id("https://crates.io", &pkgs_b);
            prop_assert_ne!(id_a, id_b);
        }

        /// Property: independent packages are always sorted alphabetically.
        #[test]
        fn prop_independent_packages_sorted_alphabetically(count in 2usize..8) {
            let td = tempdir().expect("tempdir");
            // Generate sorted unique names so we can predict the order
            let names: Vec<String> = (0..count).map(|i| format!("ind-{i:02}")).collect();
            let members: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
            write_file(
                &td.path().join("Cargo.toml"),
                &format!(
                    "[workspace]\nmembers = [{members}]\nresolver = \"2\"\n",
                    members = members.join(", ")
                ),
            );
            for name in &names {
                write_file(
                    &td.path().join(format!("{name}/Cargo.toml")),
                    &format!(
                        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
                    ),
                );
                write_file(&td.path().join(format!("{name}/src/lib.rs")), "");
            }

            let ws = build_plan(&spec_for(td.path())).expect("plan");
            let plan_names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
            let mut sorted = plan_names.clone();
            sorted.sort();
            prop_assert_eq!(plan_names, sorted, "independent packages must be alphabetical");
        }

        /// Property: for any generated DAG (diamond-ish), topo sort guarantees
        /// all deps appear before their dependents.
        #[test]
        fn prop_diamond_dag_deps_before_dependents(
            extra_leaves in 0usize..4,
        ) {
            // Build: base -> [mid-0..mid-N] -> top, plus extra independent leaves
            let mut members = vec!["base".to_string()];
            let mid_count = 2 + extra_leaves; // at least 2 middle nodes
            for i in 0..mid_count {
                members.push(format!("mid-{i}"));
            }
            members.push("top".to_string());
            for i in 0..extra_leaves {
                members.push(format!("leaf-{i}"));
            }

            let td = tempdir().expect("tempdir");
            let quoted: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
            write_file(
                &td.path().join("Cargo.toml"),
                &format!(
                    "[workspace]\nmembers = [{ms}]\nresolver = \"2\"\n",
                    ms = quoted.join(", ")
                ),
            );

            // base: no deps
            write_file(
                &td.path().join("base/Cargo.toml"),
                "[package]\nname = \"base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            );
            write_file(&td.path().join("base/src/lib.rs"), "");

            // mid-N: depends on base
            for i in 0..mid_count {
                let name = format!("mid-{i}");
                write_file(
                    &td.path().join(format!("{name}/Cargo.toml")),
                    &format!(
                        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbase = {{ path = \"../base\", version = \"0.1.0\" }}\n"
                    ),
                );
                write_file(&td.path().join(format!("{name}/src/lib.rs")), "");
            }

            // top: depends on all mid-N
            let mut top_deps = String::from("[dependencies]\n");
            for i in 0..mid_count {
                top_deps.push_str(&format!(
                    "mid-{i} = {{ path = \"../mid-{i}\", version = \"0.1.0\" }}\n"
                ));
            }
            write_file(
                &td.path().join("top/Cargo.toml"),
                &format!(
                    "[package]\nname = \"top\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{top_deps}"
                ),
            );
            write_file(&td.path().join("top/src/lib.rs"), "");

            // leaf-N: independent
            for i in 0..extra_leaves {
                let name = format!("leaf-{i}");
                write_file(
                    &td.path().join(format!("{name}/Cargo.toml")),
                    &format!(
                        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
                    ),
                );
                write_file(&td.path().join(format!("{name}/src/lib.rs")), "");
            }

            let ws = build_plan(&spec_for(td.path())).expect("plan");
            let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();

            // Verify: every dep appears before its dependent
            for pkg in &ws.plan.packages {
                let pkg_pos = names.iter().position(|n| *n == pkg.name).unwrap();
                if let Some(deps) = ws.plan.dependencies.get(&pkg.name) {
                    for dep in deps {
                        let dep_pos = names.iter().position(|n| n == dep).unwrap();
                        prop_assert!(
                            dep_pos < pkg_pos,
                            "dep {dep} (pos {dep_pos}) must come before {} (pos {pkg_pos})",
                            pkg.name
                        );
                    }
                }
            }
        }

        /// Property: the plan always contains exactly as many dependency-map entries
        /// as there are packages.
        #[test]
        fn prop_deps_map_size_equals_package_count(chain_len in 1usize..6) {
            let td = tempdir().expect("tempdir");
            let members: Vec<String> = (0..chain_len).map(|i| format!("\"q{i}\"")).collect();
            write_file(
                &td.path().join("Cargo.toml"),
                &format!(
                    "[workspace]\nmembers = [{ms}]\nresolver = \"2\"\n",
                    ms = members.join(", ")
                ),
            );
            for i in 0..chain_len {
                let name = format!("q{i}");
                let deps = if i > 0 {
                    let prev = format!("q{}", i - 1);
                    format!("\n[dependencies]\n{prev} = {{ path = \"../{prev}\", version = \"0.1.0\" }}\n")
                } else {
                    String::new()
                };
                write_file(
                    &td.path().join(format!("{name}/Cargo.toml")),
                    &format!(
                        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{deps}"
                    ),
                );
                write_file(&td.path().join(format!("{name}/src/lib.rs")), "");
            }

            let ws = build_plan(&spec_for(td.path())).expect("plan");
            prop_assert_eq!(ws.plan.packages.len(), ws.plan.dependencies.len());
        }
    }

    // ── Double diamond: base → [m1, m2] → mid → [t1, t2] → top ────

    fn create_double_diamond_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["dd-base", "dd-m1", "dd-m2", "dd-mid", "dd-t1", "dd-t2", "dd-top"]
resolver = "2"
"#,
        );
        // dd-base: no deps
        write_file(
            &root.join("dd-base/Cargo.toml"),
            r#"
[package]
name = "dd-base"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("dd-base/src/lib.rs"), "");

        // dd-m1, dd-m2: depend on dd-base
        for m in &["dd-m1", "dd-m2"] {
            write_file(
                &root.join(format!("{m}/Cargo.toml")),
                &format!(
                    r#"
[package]
name = "{m}"
version = "0.1.0"
edition = "2021"

[dependencies]
dd-base = {{ path = "../dd-base", version = "0.1.0" }}
"#
                ),
            );
            write_file(&root.join(format!("{m}/src/lib.rs")), "");
        }

        // dd-mid: depends on dd-m1 and dd-m2
        write_file(
            &root.join("dd-mid/Cargo.toml"),
            r#"
[package]
name = "dd-mid"
version = "0.1.0"
edition = "2021"

[dependencies]
dd-m1 = { path = "../dd-m1", version = "0.1.0" }
dd-m2 = { path = "../dd-m2", version = "0.1.0" }
"#,
        );
        write_file(&root.join("dd-mid/src/lib.rs"), "");

        // dd-t1, dd-t2: depend on dd-mid
        for t in &["dd-t1", "dd-t2"] {
            write_file(
                &root.join(format!("{t}/Cargo.toml")),
                &format!(
                    r#"
[package]
name = "{t}"
version = "0.1.0"
edition = "2021"

[dependencies]
dd-mid = {{ path = "../dd-mid", version = "0.1.0" }}
"#
                ),
            );
            write_file(&root.join(format!("{t}/src/lib.rs")), "");
        }

        // dd-top: depends on dd-t1 and dd-t2
        write_file(
            &root.join("dd-top/Cargo.toml"),
            r#"
[package]
name = "dd-top"
version = "0.1.0"
edition = "2021"

[dependencies]
dd-t1 = { path = "../dd-t1", version = "0.1.0" }
dd-t2 = { path = "../dd-t2", version = "0.1.0" }
"#,
        );
        write_file(&root.join("dd-top/src/lib.rs"), "");
    }

    #[test]
    fn build_plan_double_diamond_ordering() {
        let td = tempdir().expect("tempdir");
        create_double_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(ws.plan.packages.len(), 7);

        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();

        // Layer ordering: base < m1,m2 < mid < t1,t2 < top
        assert!(pos("dd-base") < pos("dd-m1"));
        assert!(pos("dd-base") < pos("dd-m2"));
        assert!(pos("dd-m1") < pos("dd-mid"));
        assert!(pos("dd-m2") < pos("dd-mid"));
        assert!(pos("dd-mid") < pos("dd-t1"));
        assert!(pos("dd-mid") < pos("dd-t2"));
        assert!(pos("dd-t1") < pos("dd-top"));
        assert!(pos("dd-t2") < pos("dd-top"));
    }

    #[test]
    fn build_plan_double_diamond_deps_map() {
        let td = tempdir().expect("tempdir");
        create_double_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert!(ws.plan.dependencies["dd-base"].is_empty());
        assert_eq!(ws.plan.dependencies["dd-m1"], vec!["dd-base".to_string()]);
        assert_eq!(ws.plan.dependencies["dd-m2"], vec!["dd-base".to_string()]);
        let mut mid_deps = ws.plan.dependencies["dd-mid"].clone();
        mid_deps.sort();
        assert_eq!(mid_deps, vec!["dd-m1".to_string(), "dd-m2".to_string()]);
        assert_eq!(ws.plan.dependencies["dd-t1"], vec!["dd-mid".to_string()]);
        assert_eq!(ws.plan.dependencies["dd-t2"], vec!["dd-mid".to_string()]);
        let mut top_deps = ws.plan.dependencies["dd-top"].clone();
        top_deps.sort();
        assert_eq!(top_deps, vec!["dd-t1".to_string(), "dd-t2".to_string()]);
    }

    #[test]
    fn snapshot_double_diamond_plan() {
        let td = tempdir().expect("tempdir");
        create_double_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("double_diamond_plan", snapshot_of(&ws));
    }

    // ── Fan-in: many crates depend on one root ──────────────────────

    #[test]
    fn build_plan_fan_in_many_dependents_on_one_root() {
        let td = tempdir().expect("tempdir");
        let fan_count = 8;
        let mut members = vec!["\"root\"".to_string()];
        for i in 0..fan_count {
            members.push(format!("\"fan-{i:02}\""));
        }
        write_file(
            &td.path().join("Cargo.toml"),
            &format!(
                "[workspace]\nmembers = [{ms}]\nresolver = \"2\"\n",
                ms = members.join(", ")
            ),
        );
        write_file(
            &td.path().join("root/Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(&td.path().join("root/src/lib.rs"), "");
        for i in 0..fan_count {
            let name = format!("fan-{i:02}");
            write_file(
                &td.path().join(format!("{name}/Cargo.toml")),
                &format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nroot = {{ path = \"../root\", version = \"0.1.0\" }}\n"
                ),
            );
            write_file(&td.path().join(format!("{name}/src/lib.rs")), "");
        }

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names[0], "root", "root must be first");
        // All fan-NN crates come after root and are sorted alphabetically
        let fans: Vec<&str> = names[1..].to_vec();
        let mut fans_sorted = fans.clone();
        fans_sorted.sort();
        assert_eq!(fans, fans_sorted);
        assert_eq!(ws.plan.packages.len(), fan_count + 1);
    }

    // ── Fan-out: one crate depends on many roots ────────────────────

    #[test]
    fn build_plan_fan_out_one_dependent_on_many() {
        let td = tempdir().expect("tempdir");
        let root_count = 5;
        let mut members = Vec::new();
        for i in 0..root_count {
            members.push(format!("\"base-{i:02}\""));
        }
        members.push("\"consumer\"".to_string());
        write_file(
            &td.path().join("Cargo.toml"),
            &format!(
                "[workspace]\nmembers = [{ms}]\nresolver = \"2\"\n",
                ms = members.join(", ")
            ),
        );
        for i in 0..root_count {
            let name = format!("base-{i:02}");
            write_file(
                &td.path().join(format!("{name}/Cargo.toml")),
                &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            );
            write_file(&td.path().join(format!("{name}/src/lib.rs")), "");
        }
        let mut consumer_deps = String::from("[dependencies]\n");
        for i in 0..root_count {
            consumer_deps.push_str(&format!(
                "base-{i:02} = {{ path = \"../base-{i:02}\", version = \"0.1.0\" }}\n"
            ));
        }
        write_file(
            &td.path().join("consumer/Cargo.toml"),
            &format!(
                "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{consumer_deps}"
            ),
        );
        write_file(&td.path().join("consumer/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // consumer must be last (it depends on all bases)
        assert_eq!(*names.last().unwrap(), "consumer");
        // All bases come before consumer, sorted alphabetically
        let bases: Vec<&str> = names[..root_count].to_vec();
        let mut bases_sorted = bases.clone();
        bases_sorted.sort();
        assert_eq!(bases, bases_sorted);
    }

    // ── Combined build-dep + runtime dep ────────────────────────────

    fn create_mixed_dep_kinds_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["build-tool", "runtime-lib", "app-mixed"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("build-tool/Cargo.toml"),
            r#"
[package]
name = "build-tool"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("build-tool/src/lib.rs"), "");
        write_file(
            &root.join("runtime-lib/Cargo.toml"),
            r#"
[package]
name = "runtime-lib"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("runtime-lib/src/lib.rs"), "");
        write_file(
            &root.join("app-mixed/Cargo.toml"),
            r#"
[package]
name = "app-mixed"
version = "0.1.0"
edition = "2021"

[dependencies]
runtime-lib = { path = "../runtime-lib", version = "0.1.0" }

[build-dependencies]
build-tool = { path = "../build-tool", version = "0.1.0" }
"#,
        );
        write_file(&root.join("app-mixed/src/lib.rs"), "");
        write_file(&root.join("app-mixed/build.rs"), "fn main() {}");
    }

    #[test]
    fn build_plan_mixed_build_and_runtime_deps() {
        let td = tempdir().expect("tempdir");
        create_mixed_dep_kinds_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();

        // Both build-tool and runtime-lib must come before app-mixed
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("build-tool") < pos("app-mixed"));
        assert!(pos("runtime-lib") < pos("app-mixed"));
        assert_eq!(ws.plan.packages.len(), 3);

        // app-mixed's deps should include both
        let mut app_deps = ws.plan.dependencies["app-mixed"].clone();
        app_deps.sort();
        assert_eq!(
            app_deps,
            vec!["build-tool".to_string(), "runtime-lib".to_string()]
        );
    }

    #[test]
    fn snapshot_mixed_dep_kinds_plan() {
        let td = tempdir().expect("tempdir");
        create_mixed_dep_kinds_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("mixed_dep_kinds_plan", snapshot_of(&ws));
    }

    // ── Dev-dep on non-publishable workspace member doesn't error ────

    #[test]
    fn build_plan_dev_dep_on_non_publishable_is_fine() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["pub-crate", "test-helper"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("test-helper/Cargo.toml"),
            r#"
[package]
name = "test-helper"
version = "0.1.0"
edition = "2021"
publish = false
"#,
        );
        write_file(&td.path().join("test-helper/src/lib.rs"), "");
        write_file(
            &td.path().join("pub-crate/Cargo.toml"),
            r#"
[package]
name = "pub-crate"
version = "0.1.0"
edition = "2021"

[dev-dependencies]
test-helper = { path = "../test-helper", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("pub-crate/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan should succeed");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["pub-crate"]);
        assert_eq!(ws.skipped.len(), 1);
        assert_eq!(ws.skipped[0].name, "test-helper");
        // pub-crate has no normal/build deps in the plan
        assert!(ws.plan.dependencies["pub-crate"].is_empty());
    }

    // ── Dev-dep would-be cycle (not a real cycle since dev-deps excluded) ──

    #[test]
    fn build_plan_dev_dep_would_be_cycle_is_not_error() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["crate-x", "crate-y"]
resolver = "2"
"#,
        );
        // crate-x depends on crate-y (normal)
        write_file(
            &td.path().join("crate-x/Cargo.toml"),
            r#"
[package]
name = "crate-x"
version = "0.1.0"
edition = "2021"

[dependencies]
crate-y = { path = "../crate-y", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("crate-x/src/lib.rs"), "");
        // crate-y dev-depends on crate-x (would be cycle if counted)
        write_file(
            &td.path().join("crate-y/Cargo.toml"),
            r#"
[package]
name = "crate-y"
version = "0.1.0"
edition = "2021"

[dev-dependencies]
crate-x = { path = "../crate-x", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("crate-y/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan should succeed (dev-dep cycle ok)");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // y must come before x (x depends on y normally)
        assert_eq!(names, vec!["crate-y", "crate-x"]);
        assert!(ws.plan.dependencies["crate-y"].is_empty());
        assert_eq!(ws.plan.dependencies["crate-x"], vec!["crate-y".to_string()]);
    }

    // ── Explicit publish = ["crates-io"] behaves like publish = None ──

    #[test]
    fn build_plan_explicit_crates_io_publish_list() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["explicit-pub"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("explicit-pub/Cargo.toml"),
            r#"
[package]
name = "explicit-pub"
version = "1.0.0"
edition = "2021"
publish = ["crates-io"]
"#,
        );
        write_file(&td.path().join("explicit-pub/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages.len(), 1);
        assert_eq!(ws.plan.packages[0].name, "explicit-pub");
        assert!(ws.skipped.is_empty());
    }

    // ── Multiple registries in publish list ──────────────────────────

    #[test]
    fn build_plan_multi_registry_publish_list() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["multi-reg"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("multi-reg/Cargo.toml"),
            r#"
[package]
name = "multi-reg"
version = "0.5.0"
edition = "2021"
publish = ["crates-io", "private-reg"]
"#,
        );
        write_file(&td.path().join("multi-reg/src/lib.rs"), "");

        // Included for crates-io
        let ws_cio = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws_cio.plan.packages.len(), 1);
        assert_eq!(ws_cio.plan.packages[0].name, "multi-reg");

        // Included for private-reg
        let spec_priv = ReleaseSpec {
            manifest_path: td.path().join("Cargo.toml"),
            registry: Registry {
                name: "private-reg".to_string(),
                api_base: "https://private.example.com".to_string(),
                index_base: None,
            },
            selected_packages: None,
        };
        let ws_priv = build_plan(&spec_priv).expect("plan");
        assert_eq!(ws_priv.plan.packages.len(), 1);

        // Excluded for unknown registry
        let spec_other = ReleaseSpec {
            manifest_path: td.path().join("Cargo.toml"),
            registry: Registry {
                name: "other-reg".to_string(),
                api_base: "https://other.example.com".to_string(),
                index_base: None,
            },
            selected_packages: None,
        };
        let ws_other = build_plan(&spec_other).expect("plan");
        assert!(ws_other.plan.packages.is_empty());
        assert_eq!(ws_other.skipped.len(), 1);
    }

    // ── Pre-release and build-metadata versions ─────────────────────

    #[test]
    fn build_plan_prerelease_versions() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["pre-alpha", "pre-beta"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("pre-alpha/Cargo.toml"),
            r#"
[package]
name = "pre-alpha"
version = "0.1.0-alpha.1"
edition = "2021"
"#,
        );
        write_file(&td.path().join("pre-alpha/src/lib.rs"), "");
        write_file(
            &td.path().join("pre-beta/Cargo.toml"),
            r#"
[package]
name = "pre-beta"
version = "2.0.0-rc.3"
edition = "2021"

[dependencies]
pre-alpha = { path = "../pre-alpha", version = "0.1.0-alpha.1" }
"#,
        );
        write_file(&td.path().join("pre-beta/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages[0].name, "pre-alpha");
        assert_eq!(ws.plan.packages[0].version, "0.1.0-alpha.1");
        assert_eq!(ws.plan.packages[1].name, "pre-beta");
        assert_eq!(ws.plan.packages[1].version, "2.0.0-rc.3");
    }

    // ── Plan ID changes when version bumps ──────────────────────────

    #[test]
    fn build_plan_id_changes_on_version_bump() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["bump-me"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("bump-me/Cargo.toml"),
            r#"
[package]
name = "bump-me"
version = "1.0.0"
edition = "2021"
"#,
        );
        write_file(&td.path().join("bump-me/src/lib.rs"), "");

        let ws1 = build_plan(&spec_for(td.path())).expect("plan");

        // Bump version
        write_file(
            &td.path().join("bump-me/Cargo.toml"),
            r#"
[package]
name = "bump-me"
version = "1.1.0"
edition = "2021"
"#,
        );

        let ws2 = build_plan(&spec_for(td.path())).expect("plan");
        assert_ne!(ws1.plan.plan_id, ws2.plan.plan_id);
        assert_eq!(ws2.plan.packages[0].version, "1.1.0");
    }

    // ── Selecting middle of diamond pulls both leaves ────────────────

    #[test]
    fn build_plan_selecting_diamond_middle_pulls_transitive() {
        let td = tempdir().expect("tempdir");
        create_double_diamond_workspace(td.path());

        // Select dd-mid → should pull in dd-m1, dd-m2, dd-base
        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["dd-mid".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"dd-base"));
        assert!(names.contains(&"dd-m1"));
        assert!(names.contains(&"dd-m2"));
        assert!(names.contains(&"dd-mid"));
        assert!(!names.contains(&"dd-t1"));
        assert!(!names.contains(&"dd-t2"));
        assert!(!names.contains(&"dd-top"));
        assert_eq!(names.len(), 4);
    }

    // ── Selecting top of double diamond pulls everything ─────────────

    #[test]
    fn build_plan_selecting_double_diamond_top_pulls_all() {
        let td = tempdir().expect("tempdir");
        create_double_diamond_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["dd-top".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        assert_eq!(ws.plan.packages.len(), 7);
    }

    // ── W-shape graph: two independent diamonds sharing nothing ──────

    #[test]
    fn build_plan_w_shape_two_independent_diamonds() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["l-base", "l-mid", "l-top", "r-base", "r-mid", "r-top"]
resolver = "2"
"#,
        );
        // Left diamond: l-base → l-mid → l-top
        write_file(
            &td.path().join("l-base/Cargo.toml"),
            "[package]\nname = \"l-base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(&td.path().join("l-base/src/lib.rs"), "");
        write_file(
            &td.path().join("l-mid/Cargo.toml"),
            "[package]\nname = \"l-mid\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nl-base = { path = \"../l-base\", version = \"0.1.0\" }\n",
        );
        write_file(&td.path().join("l-mid/src/lib.rs"), "");
        write_file(
            &td.path().join("l-top/Cargo.toml"),
            "[package]\nname = \"l-top\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nl-mid = { path = \"../l-mid\", version = \"0.1.0\" }\n",
        );
        write_file(&td.path().join("l-top/src/lib.rs"), "");
        // Right chain: r-base → r-mid → r-top
        write_file(
            &td.path().join("r-base/Cargo.toml"),
            "[package]\nname = \"r-base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(&td.path().join("r-base/src/lib.rs"), "");
        write_file(
            &td.path().join("r-mid/Cargo.toml"),
            "[package]\nname = \"r-mid\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nr-base = { path = \"../r-base\", version = \"0.1.0\" }\n",
        );
        write_file(&td.path().join("r-mid/src/lib.rs"), "");
        write_file(
            &td.path().join("r-top/Cargo.toml"),
            "[package]\nname = \"r-top\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nr-mid = { path = \"../r-mid\", version = \"0.1.0\" }\n",
        );
        write_file(&td.path().join("r-top/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(ws.plan.packages.len(), 6);

        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        // Left chain ordering
        assert!(pos("l-base") < pos("l-mid"));
        assert!(pos("l-mid") < pos("l-top"));
        // Right chain ordering
        assert!(pos("r-base") < pos("r-mid"));
        assert!(pos("r-mid") < pos("r-top"));
    }

    // ── Selection from W-shape picks only one subgraph ───────────────

    #[test]
    fn build_plan_w_shape_selecting_one_chain_excludes_other() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["l-base", "l-top", "r-base", "r-top"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("l-base/Cargo.toml"),
            "[package]\nname = \"l-base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(&td.path().join("l-base/src/lib.rs"), "");
        write_file(
            &td.path().join("l-top/Cargo.toml"),
            "[package]\nname = \"l-top\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nl-base = { path = \"../l-base\", version = \"0.1.0\" }\n",
        );
        write_file(&td.path().join("l-top/src/lib.rs"), "");
        write_file(
            &td.path().join("r-base/Cargo.toml"),
            "[package]\nname = \"r-base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(&td.path().join("r-base/src/lib.rs"), "");
        write_file(
            &td.path().join("r-top/Cargo.toml"),
            "[package]\nname = \"r-top\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nr-base = { path = \"../r-base\", version = \"0.1.0\" }\n",
        );
        write_file(&td.path().join("r-top/src/lib.rs"), "");

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["l-top".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["l-base", "l-top"]);
    }

    // ── Plan with workspace-level version inheritance ────────────────

    #[test]
    fn build_plan_workspace_inherited_version() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["inherited"]
resolver = "2"

[workspace.package]
version = "3.7.2"
edition = "2021"
"#,
        );
        write_file(
            &td.path().join("inherited/Cargo.toml"),
            r#"
[package]
name = "inherited"
version.workspace = true
edition.workspace = true
"#,
        );
        write_file(&td.path().join("inherited/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages.len(), 1);
        assert_eq!(ws.plan.packages[0].name, "inherited");
        assert_eq!(ws.plan.packages[0].version, "3.7.2");
    }

    // ── Skipped package reason strings ──────────────────────────────

    #[test]
    fn build_plan_skipped_reasons_are_descriptive() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["pub-false", "pub-other-reg", "pub-ok"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("pub-false/Cargo.toml"),
            "[package]\nname = \"pub-false\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
        );
        write_file(&td.path().join("pub-false/src/lib.rs"), "");
        write_file(
            &td.path().join("pub-other-reg/Cargo.toml"),
            "[package]\nname = \"pub-other-reg\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = [\"other\"]\n",
        );
        write_file(&td.path().join("pub-other-reg/src/lib.rs"), "");
        write_file(
            &td.path().join("pub-ok/Cargo.toml"),
            "[package]\nname = \"pub-ok\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(&td.path().join("pub-ok/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages.len(), 1);
        assert_eq!(ws.skipped.len(), 2);

        let pub_false_skip = ws.skipped.iter().find(|s| s.name == "pub-false").unwrap();
        assert!(
            pub_false_skip.reason.contains("publish = false"),
            "reason: {}",
            pub_false_skip.reason
        );

        let pub_other_skip = ws
            .skipped
            .iter()
            .find(|s| s.name == "pub-other-reg")
            .unwrap();
        assert!(
            pub_other_skip.reason.contains("registry not in list"),
            "reason: {}",
            pub_other_skip.reason
        );
    }

    // ── compute_plan_id: single pkg vs two identical pkgs ───────────

    #[test]
    fn compute_plan_id_differs_for_single_vs_duplicated_package() {
        let pkg = PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: PathBuf::from("foo/Cargo.toml"),
        };
        let id_one = compute_plan_id("https://crates.io", std::slice::from_ref(&pkg));
        let id_two = compute_plan_id("https://crates.io", &[pkg.clone(), pkg]);
        assert_ne!(id_one, id_two);
    }

    // ── Snapshot for double diamond with selection ────────────────────

    #[test]
    fn snapshot_double_diamond_selected_mid() {
        let td = tempdir().expect("tempdir");
        create_double_diamond_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["dd-mid".to_string()]);
        let ws = build_plan(&spec).expect("plan");
        insta::assert_yaml_snapshot!("double_diamond_selected_mid", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_prerelease_versions() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["pre-alpha", "pre-beta"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("pre-alpha/Cargo.toml"),
            r#"
[package]
name = "pre-alpha"
version = "0.1.0-alpha.1"
edition = "2021"
"#,
        );
        write_file(&td.path().join("pre-alpha/src/lib.rs"), "");
        write_file(
            &td.path().join("pre-beta/Cargo.toml"),
            r#"
[package]
name = "pre-beta"
version = "2.0.0-rc.3"
edition = "2021"

[dependencies]
pre-alpha = { path = "../pre-alpha", version = "0.1.0-alpha.1" }
"#,
        );
        write_file(&td.path().join("pre-beta/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("prerelease_versions_plan", snapshot_of(&ws));
    }

    #[test]
    fn snapshot_dev_dep_cycle_plan() {
        let td = tempdir().expect("tempdir");
        write_file(
            &td.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["crate-x", "crate-y"]
resolver = "2"
"#,
        );
        write_file(
            &td.path().join("crate-x/Cargo.toml"),
            r#"
[package]
name = "crate-x"
version = "0.1.0"
edition = "2021"

[dependencies]
crate-y = { path = "../crate-y", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("crate-x/src/lib.rs"), "");
        write_file(
            &td.path().join("crate-y/Cargo.toml"),
            r#"
[package]
name = "crate-y"
version = "0.1.0"
edition = "2021"

[dev-dependencies]
crate-x = { path = "../crate-x", version = "0.1.0" }
"#,
        );
        write_file(&td.path().join("crate-y/src/lib.rs"), "");

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        insta::assert_yaml_snapshot!("dev_dep_cycle_plan", snapshot_of(&ws));
    }

    // ── error message quality snapshots ──────────────────────────────────

    #[test]
    fn snapshot_error_message_missing_manifest() {
        let spec = ReleaseSpec {
            manifest_path: Path::new("nonexistent-dir").join("Cargo.toml"),
            registry: Registry::crates_io(),
            selected_packages: None,
        };
        let err = build_plan(&spec).expect_err("must fail");
        insta::assert_snapshot!("error_msg_missing_manifest", format!("{err:#}"));
    }

    #[test]
    fn snapshot_error_message_unknown_selected_package() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["totally-unknown-crate".to_string()]);
        let err = build_plan(&spec).expect_err("must fail");
        insta::assert_snapshot!("error_msg_unknown_selected_package", format!("{err:#}"));
    }

    #[test]
    fn snapshot_error_message_non_publishable_dep() {
        let td = tempdir().expect("tempdir");
        create_workspace_with_npdep(td.path(), true);
        let err = build_plan(&spec_for(td.path())).expect_err("must fail");
        insta::assert_snapshot!("error_msg_non_publishable_dep", format!("{err:#}"));
    }

    #[test]
    fn snapshot_error_message_selecting_non_publishable() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["c".to_string()]);
        let err = build_plan(&spec).expect_err("must fail");
        insta::assert_snapshot!("error_msg_selecting_non_publishable", format!("{err:#}"));
    }

    #[test]
    fn snapshot_error_message_cycle_detection() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let metadata = MetadataCommand::new()
            .manifest_path(td.path().join("Cargo.toml"))
            .exec()
            .expect("metadata");

        let pkg_map = metadata
            .packages
            .iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<PackageId, &cargo_metadata::Package>>();
        let mut by_name = BTreeMap::<String, PackageId>::new();
        for pkg in &metadata.packages {
            by_name.insert(pkg.name.to_string(), pkg.id.clone());
        }

        let a = by_name.get("a").expect("a").clone();
        let b = by_name.get("b").expect("b").clone();

        let included = [a.clone(), b.clone()].into_iter().collect::<BTreeSet<_>>();
        let deps_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);
        let dependents_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);

        let err = topo_sort(&included, &deps_of, &dependents_of, &pkg_map).expect_err("cycle");
        insta::assert_snapshot!("error_msg_cycle_detection", format!("{err:#}"));
    }
}
