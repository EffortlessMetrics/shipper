//! Workspace analysis and topological plan generation.
//!
//! Note: this file is the unchanged historical `plan.rs`, relocated to
//! `plan/mod.rs` to enable the `plan/` layer directory. A later PR will
//! replace its contents with the absorbed `shipper-plan` microcrate code.

pub mod levels;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
#[cfg(not(feature = "micro-cargo"))]
use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, PackageId};
#[cfg(feature = "micro-cargo")]
use cargo_metadata::{DependencyKind, Metadata, PackageId};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::types::{PlannedPackage, ReleasePlan, ReleaseSpec};

#[derive(Debug, Clone)]
pub struct SkippedPackage {
    pub name: String,
    pub version: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct PlannedWorkspace {
    pub workspace_root: PathBuf,
    pub plan: ReleasePlan,
    pub skipped: Vec<SkippedPackage>,
}

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
            plan_version: crate::state::CURRENT_PLAN_VERSION.to_string(),
            plan_id,
            created_at: Utc::now(),
            registry: spec.registry.clone(),
            packages,
            dependencies,
        },
        skipped,
    })
}

#[cfg(feature = "micro-cargo")]
fn load_metadata(manifest_path: &Path) -> Result<Metadata> {
    shipper_cargo::load_metadata(manifest_path)
}

#[cfg(not(feature = "micro-cargo"))]
fn load_metadata(manifest_path: &Path) -> Result<Metadata> {
    let mut cmd = MetadataCommand::new();
    cmd.manifest_path(manifest_path);
    cmd.exec().context("failed to execute cargo metadata")
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
    use tempfile::tempdir;

    use super::*;
    use crate::types::Registry;

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
    }

    // -----------------------------------------------------------------------
    // Helper: create a diamond workspace (base → left+right → apex)
    // -----------------------------------------------------------------------
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

    // -----------------------------------------------------------------------
    // Helper: create a deep chain workspace (l0 → l1 → l2 → l3 → l4)
    // -----------------------------------------------------------------------
    fn create_deep_chain_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["l0", "l1", "l2", "l3", "l4"]
resolver = "2"
"#,
        );

        write_file(
            &root.join("l0/Cargo.toml"),
            r#"
[package]
name = "l0"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("l0/src/lib.rs"), "pub fn l0() {}\n");

        for i in 1..=4 {
            let name = format!("l{i}");
            let dep_name = format!("l{}", i - 1);
            write_file(
                &root.join(format!("{name}/Cargo.toml")),
                &format!(
                    r#"
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
{dep_name} = {{ path = "../{dep_name}", version = "0.1.0" }}
"#
                ),
            );
            write_file(
                &root.join(format!("{name}/src/lib.rs")),
                &format!("pub fn {name}() {{}}\n"),
            );
        }
    }

    // -----------------------------------------------------------------------
    // Helper: single-package workspace
    // -----------------------------------------------------------------------
    fn create_single_package_workspace(root: &Path) {
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
version = "1.0.0"
edition = "2021"
"#,
        );
        write_file(&root.join("only/src/lib.rs"), "pub fn only() {}\n");
    }

    // =======================================================================
    // Diamond dependency graph tests
    // =======================================================================

    #[test]
    fn diamond_dependency_ordering_is_correct() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names.len(), 4);

        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        // base before left and right
        assert!(pos("base") < pos("left"));
        assert!(pos("base") < pos("right"));
        // left and right before apex
        assert!(pos("left") < pos("apex"));
        assert!(pos("right") < pos("apex"));
        // alphabetical among peers
        assert!(pos("left") < pos("right"));
    }

    #[test]
    fn diamond_dependency_map_is_correct() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let deps = &ws.plan.dependencies;

        assert!(deps.get("base").unwrap().is_empty());
        assert_eq!(deps.get("left").unwrap(), &vec!["base".to_string()]);
        assert_eq!(deps.get("right").unwrap(), &vec!["base".to_string()]);

        let apex_deps = deps.get("apex").unwrap();
        assert!(apex_deps.contains(&"left".to_string()));
        assert!(apex_deps.contains(&"right".to_string()));
        assert_eq!(apex_deps.len(), 2);
    }

    // =======================================================================
    // Deep chain tests
    // =======================================================================

    #[test]
    fn deep_chain_preserves_linear_order() {
        let td = tempdir().expect("tempdir");
        create_deep_chain_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["l0", "l1", "l2", "l3", "l4"]);
    }

    #[test]
    fn deep_chain_selecting_tip_pulls_entire_chain() {
        let td = tempdir().expect("tempdir");
        create_deep_chain_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["l4".to_string()]);

        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["l0", "l1", "l2", "l3", "l4"]);
    }

    #[test]
    fn deep_chain_selecting_middle_pulls_prefix_only() {
        let td = tempdir().expect("tempdir");
        create_deep_chain_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["l2".to_string()]);

        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["l0", "l1", "l2"]);
    }

    // =======================================================================
    // Cycle detection (unit-level via topo_sort)
    // =======================================================================

    #[test]
    fn topo_sort_detects_three_node_cycle() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());
        let manifest = td.path().join("Cargo.toml");
        let metadata = MetadataCommand::new()
            .manifest_path(&manifest)
            .exec()
            .expect("metadata");

        let pkg_map: BTreeMap<PackageId, &cargo_metadata::Package> = metadata
            .packages
            .iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let mut by_name = BTreeMap::<String, PackageId>::new();
        for pkg in &metadata.packages {
            by_name.insert(pkg.name.to_string(), pkg.id.clone());
        }

        let a = by_name["a"].clone();
        let b = by_name["b"].clone();
        let z = by_name["zeta"].clone();

        // a→b→z→a
        let included = [a.clone(), b.clone(), z.clone()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let deps_of = BTreeMap::from([
            (a.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (b.clone(), [z.clone()].into_iter().collect::<BTreeSet<_>>()),
            (z.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);
        let dependents_of = BTreeMap::from([
            (b.clone(), [a.clone()].into_iter().collect::<BTreeSet<_>>()),
            (z.clone(), [b.clone()].into_iter().collect::<BTreeSet<_>>()),
            (a.clone(), [z.clone()].into_iter().collect::<BTreeSet<_>>()),
        ]);

        let err = topo_sort(&included, &deps_of, &dependents_of, &pkg_map).expect_err("cycle");
        assert!(format!("{err:#}").contains("dependency cycle detected"));
    }

    #[test]
    fn topo_sort_succeeds_for_empty_set() {
        let included = BTreeSet::new();
        let deps_of = BTreeMap::new();
        let dependents_of = BTreeMap::new();
        let pkg_map = BTreeMap::new();

        let result = topo_sort(&included, &deps_of, &dependents_of, &pkg_map).expect("empty ok");
        assert!(result.is_empty());
    }

    // =======================================================================
    // Single-package workspace
    // =======================================================================

    #[test]
    fn single_package_workspace_produces_one_entry_plan() {
        let td = tempdir().expect("tempdir");
        create_single_package_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.packages.len(), 1);
        assert_eq!(ws.plan.packages[0].name, "only");
        assert_eq!(ws.plan.packages[0].version, "1.0.0");
        assert!(ws.skipped.is_empty());
    }

    #[test]
    fn single_package_workspace_dependencies_map_is_empty_vec() {
        let td = tempdir().expect("tempdir");
        create_single_package_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let deps = ws.plan.dependencies.get("only").expect("only in deps map");
        assert!(deps.is_empty());
    }

    // =======================================================================
    // All-independent packages (no inter-workspace deps)
    // =======================================================================

    #[test]
    fn all_independent_packages_sorted_alphabetically() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["charlie", "alpha", "bravo"]
resolver = "2"
"#,
        );

        for name in &["charlie", "alpha", "bravo"] {
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
            write_file(
                &root.join(format!("{name}/src/lib.rs")),
                &format!("pub fn {name}() {{}}\n"),
            );
        }

        let ws = build_plan(&spec_for(root)).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    // =======================================================================
    // Plan determinism
    // =======================================================================

    #[test]
    fn plan_is_deterministic_across_multiple_invocations() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());
        let spec = spec_for(td.path());

        let plans: Vec<_> = (0..5).map(|_| build_plan(&spec).expect("plan")).collect();

        let first_names: Vec<&str> = plans[0]
            .plan
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        for (i, p) in plans.iter().enumerate().skip(1) {
            let names: Vec<&str> = p.plan.packages.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(first_names, names, "plan order differs at invocation {i}");
        }
    }

    // =======================================================================
    // Plan ID / hash stability
    // =======================================================================

    #[test]
    fn plan_id_changes_when_version_changes() {
        let pkgs_v1 = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: Path::new("x").join("foo.toml"),
        }];
        let pkgs_v2 = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "2.0.0".to_string(),
            manifest_path: Path::new("x").join("foo.toml"),
        }];

        let id1 = compute_plan_id("https://crates.io", &pkgs_v1);
        let id2 = compute_plan_id("https://crates.io", &pkgs_v2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn plan_id_changes_when_package_added() {
        let pkgs_one = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: Path::new("x").join("foo.toml"),
        }];
        let pkgs_two = vec![
            PlannedPackage {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: Path::new("x").join("foo.toml"),
            },
            PlannedPackage {
                name: "bar".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: Path::new("x").join("bar.toml"),
            },
        ];

        let id1 = compute_plan_id("https://crates.io", &pkgs_one);
        let id2 = compute_plan_id("https://crates.io", &pkgs_two);
        assert_ne!(id1, id2);
    }

    #[test]
    fn plan_id_changes_when_registry_differs() {
        let pkgs = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: Path::new("x").join("foo.toml"),
        }];

        let id1 = compute_plan_id("https://crates.io", &pkgs);
        let id2 = compute_plan_id("https://my-registry.example.com", &pkgs);
        assert_ne!(id1, id2);
    }

    #[test]
    fn plan_id_is_insensitive_to_manifest_path() {
        let pkgs_a = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: Path::new("/path/a").join("foo.toml"),
        }];
        let pkgs_b = vec![PlannedPackage {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: Path::new("/completely/different").join("foo.toml"),
        }];

        let id1 = compute_plan_id("https://crates.io", &pkgs_a);
        let id2 = compute_plan_id("https://crates.io", &pkgs_b);
        assert_eq!(id1, id2, "plan_id should not depend on manifest_path");
    }

    #[test]
    fn plan_id_depends_on_package_order() {
        let pkgs_ab = vec![
            PlannedPackage {
                name: "a".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: Path::new("x").join("a.toml"),
            },
            PlannedPackage {
                name: "b".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: Path::new("x").join("b.toml"),
            },
        ];
        let pkgs_ba = vec![
            PlannedPackage {
                name: "b".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: Path::new("x").join("b.toml"),
            },
            PlannedPackage {
                name: "a".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: Path::new("x").join("a.toml"),
            },
        ];

        let id_ab = compute_plan_id("https://crates.io", &pkgs_ab);
        let id_ba = compute_plan_id("https://crates.io", &pkgs_ba);
        assert_ne!(id_ab, id_ba, "reordering packages should change plan_id");
    }

    #[test]
    fn plan_id_for_empty_packages_is_valid_hex() {
        let id = compute_plan_id("https://crates.io", &[]);
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // =======================================================================
    // Package filtering edge cases
    // =======================================================================

    #[test]
    fn selecting_multiple_packages_unions_their_transitive_deps() {
        let td = tempdir().expect("tempdir");
        create_diamond_workspace(td.path());

        let mut spec = spec_for(td.path());
        spec.selected_packages = Some(vec!["left".to_string(), "right".to_string()]);

        let ws = build_plan(&spec).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // Both left and right depend on base; apex is NOT included
        assert_eq!(names, vec!["base", "left", "right"]);
    }

    #[test]
    fn selecting_non_publishable_package_errors() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let mut spec = spec_for(td.path());
        // "c" has publish = false
        spec.selected_packages = Some(vec!["c".to_string()]);

        let err = build_plan(&spec).expect_err("must fail");
        assert!(
            format!("{err:#}").contains("selected package not found or not publishable"),
            "error: {err:#}"
        );
    }

    // =======================================================================
    // Skipped packages tracking
    // =======================================================================

    #[test]
    fn skipped_packages_includes_publish_false() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let skipped_names: Vec<&str> = ws.skipped.iter().map(|s| s.name.as_str()).collect();
        assert!(skipped_names.contains(&"c"), "c has publish=false");
        assert!(
            skipped_names.contains(&"d"),
            "d has publish=[private-reg] which excludes crates-io"
        );
    }

    #[test]
    fn skipped_packages_reason_for_publish_false() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let c_skip = ws
            .skipped
            .iter()
            .find(|s| s.name == "c")
            .expect("c skipped");
        assert!(
            c_skip.reason.contains("publish = false"),
            "reason: {}",
            c_skip.reason
        );
    }

    #[test]
    fn skipped_packages_reason_for_registry_mismatch() {
        let td = tempdir().expect("tempdir");
        create_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        let d_skip = ws
            .skipped
            .iter()
            .find(|s| s.name == "d")
            .expect("d skipped");
        assert!(
            d_skip.reason.contains("registry not in list"),
            "reason: {}",
            d_skip.reason
        );
    }

    // =======================================================================
    // publish_allowed unit tests
    // =======================================================================

    #[test]
    fn publish_allowed_with_matching_registry() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["pkg"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("pkg/Cargo.toml"),
            r#"
[package]
name = "pkg"
version = "0.1.0"
edition = "2021"
publish = ["private-reg"]
"#,
        );
        write_file(&root.join("pkg/src/lib.rs"), "pub fn pkg() {}\n");

        let mut spec = spec_for(root);
        spec.registry = Registry {
            name: "private-reg".to_string(),
            api_base: "https://private.example.com".to_string(),
            index_base: None,
        };

        let ws = build_plan(&spec).expect("plan");
        assert_eq!(ws.plan.packages.len(), 1);
        assert_eq!(ws.plan.packages[0].name, "pkg");
        assert!(ws.skipped.is_empty());
    }

    #[test]
    fn publish_allowed_with_non_matching_registry_skips() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["pkg"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("pkg/Cargo.toml"),
            r#"
[package]
name = "pkg"
version = "0.1.0"
edition = "2021"
publish = ["some-other-registry"]
"#,
        );
        write_file(&root.join("pkg/src/lib.rs"), "pub fn pkg() {}\n");

        let ws = build_plan(&spec_for(root)).expect("plan");
        assert!(ws.plan.packages.is_empty());
        assert_eq!(ws.skipped.len(), 1);
        assert_eq!(ws.skipped[0].name, "pkg");
    }

    // =======================================================================
    // Build-dependency handling
    // =======================================================================

    #[test]
    fn build_dependency_is_included_in_plan_order() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["codegen", "consumer"]
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
        write_file(&root.join("codegen/src/lib.rs"), "pub fn codegen() {}\n");

        write_file(
            &root.join("consumer/Cargo.toml"),
            r#"
[package]
name = "consumer"
version = "0.1.0"
edition = "2021"

[build-dependencies]
codegen = { path = "../codegen", version = "0.1.0" }
"#,
        );
        write_file(&root.join("consumer/src/lib.rs"), "pub fn consumer() {}\n");
        write_file(&root.join("consumer/build.rs"), "fn main() {}\n");

        let ws = build_plan(&spec_for(root)).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["codegen", "consumer"]);

        let consumer_deps = ws.plan.dependencies.get("consumer").unwrap();
        assert!(consumer_deps.contains(&"codegen".to_string()));
    }

    // =======================================================================
    // Dev-dependency is NOT an ordering constraint
    // =======================================================================

    #[test]
    fn dev_dependency_does_not_create_ordering_constraint() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["helper", "main-lib"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("helper/Cargo.toml"),
            r#"
[package]
name = "helper"
version = "0.1.0"
edition = "2021"
"#,
        );
        write_file(&root.join("helper/src/lib.rs"), "pub fn helper() {}\n");

        write_file(
            &root.join("main-lib/Cargo.toml"),
            r#"
[package]
name = "main-lib"
version = "0.1.0"
edition = "2021"

[dev-dependencies]
helper = { path = "../helper", version = "0.1.0" }
"#,
        );
        write_file(&root.join("main-lib/src/lib.rs"), "pub fn main_lib() {}\n");

        let ws = build_plan(&spec_for(root)).expect("plan");
        let names: Vec<&str> = ws.plan.packages.iter().map(|p| p.name.as_str()).collect();
        // Both packages should appear, sorted alphabetically since no normal/build dep edge
        assert_eq!(names, vec!["helper", "main-lib"]);

        // main-lib should have no plan-level dependencies on helper
        let main_deps = ws.plan.dependencies.get("main-lib").unwrap();
        assert!(
            !main_deps.contains(&"helper".to_string()),
            "dev-dep should not appear in plan dependencies"
        );
    }

    // =======================================================================
    // Invalid manifest handling
    // =======================================================================

    #[test]
    fn build_plan_errors_for_malformed_manifest() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(&root.join("Cargo.toml"), "this is not valid TOML {{{{");

        let err = build_plan(&spec_for(root)).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to execute cargo metadata") || msg.contains("could not parse"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn build_plan_errors_for_nonexistent_workspace_member() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["ghost"]
resolver = "2"
"#,
        );
        // Don't create the "ghost" directory/manifest at all

        let err = build_plan(&spec_for(root)).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to execute cargo metadata") || msg.contains("failed to read"),
            "unexpected error: {msg}"
        );
    }

    // =======================================================================
    // Workspace where ALL packages are non-publishable
    // =======================================================================

    #[test]
    fn all_packages_non_publishable_produces_empty_plan() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["x", "y"]
resolver = "2"
"#,
        );

        for name in &["x", "y"] {
            write_file(
                &root.join(format!("{name}/Cargo.toml")),
                &format!(
                    r#"
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
publish = false
"#
                ),
            );
            write_file(
                &root.join(format!("{name}/src/lib.rs")),
                &format!("pub fn {name}() {{}}\n"),
            );
        }

        let ws = build_plan(&spec_for(root)).expect("plan");
        assert!(ws.plan.packages.is_empty());
        assert_eq!(ws.skipped.len(), 2);
    }

    // =======================================================================
    // Mixed versions in the plan
    // =======================================================================

    #[test]
    fn packages_with_different_versions_appear_correctly() {
        let td = tempdir().expect("tempdir");
        let root = td.path();

        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["core-lib", "ext-lib"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("core-lib/Cargo.toml"),
            r#"
[package]
name = "core-lib"
version = "3.2.1"
edition = "2021"
"#,
        );
        write_file(&root.join("core-lib/src/lib.rs"), "pub fn core_lib() {}\n");

        write_file(
            &root.join("ext-lib/Cargo.toml"),
            r#"
[package]
name = "ext-lib"
version = "0.0.1-alpha"
edition = "2021"

[dependencies]
core-lib = { path = "../core-lib", version = "3.2.1" }
"#,
        );
        write_file(&root.join("ext-lib/src/lib.rs"), "pub fn ext_lib() {}\n");

        let ws = build_plan(&spec_for(root)).expect("plan");
        assert_eq!(ws.plan.packages.len(), 2);
        assert_eq!(ws.plan.packages[0].name, "core-lib");
        assert_eq!(ws.plan.packages[0].version, "3.2.1");
        assert_eq!(ws.plan.packages[1].name, "ext-lib");
        assert_eq!(ws.plan.packages[1].version, "0.0.1-alpha");
    }

    // =======================================================================
    // Plan version field
    // =======================================================================

    #[test]
    fn plan_version_is_set_correctly() {
        let td = tempdir().expect("tempdir");
        create_single_package_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        assert_eq!(ws.plan.plan_version, crate::state::CURRENT_PLAN_VERSION,);
    }

    // =======================================================================
    // workspace_root is set correctly
    // =======================================================================

    #[test]
    fn workspace_root_matches_manifest_directory() {
        let td = tempdir().expect("tempdir");
        create_single_package_workspace(td.path());

        let ws = build_plan(&spec_for(td.path())).expect("plan");
        // workspace_root should be the canonical path of the tempdir
        assert!(
            ws.workspace_root.ends_with(td.path().file_name().unwrap())
                || ws.workspace_root == td.path().canonicalize().unwrap_or_default(),
            "workspace_root {:?} should correspond to {:?}",
            ws.workspace_root,
            td.path()
        );
    }
}
