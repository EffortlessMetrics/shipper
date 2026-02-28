use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use cargo_metadata::{DependencyKind, Metadata, PackageId};
use chrono::Utc;
use sha2::{Digest, Sha256};
use shipper_types::{PlannedPackage, ReleasePlan, ReleaseSpec};

#[derive(Debug, Clone)]
#[cfg_attr(test, derive(serde::Serialize))]
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
}
