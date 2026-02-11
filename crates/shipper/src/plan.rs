use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, PackageId};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::types::{PlannedPackage, PublishLevel, ReleasePlan, ReleaseSpec};

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

    // Topological sort on included nodes.
    let order = topo_sort(&included, &deps_of, &dependents_of, &pkg_map)?;

    let packages: Vec<PlannedPackage> = order
        .into_iter()
        .map(|id| {
            let pkg = pkg_map.get(&id).expect("pkg exists");
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
            plan_id,
            created_at: Utc::now(),
            registry: spec.registry.clone(),
            packages,
            dependencies,
        },
        skipped,
    })
}

fn load_metadata(manifest_path: &PathBuf) -> Result<Metadata> {
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

impl ReleasePlan {
    /// Group packages by dependency level for parallel publishing.
    ///
    /// Packages at the same level have no dependencies on each other and can be
    /// published in parallel. Level 0 packages have no dependencies on other packages
    /// in the plan. Level N packages depend only on packages in levels < N.
    ///
    /// This method uses the `dependencies` field of the ReleasePlan to determine levels.
    pub fn group_by_levels(&self) -> Vec<PublishLevel> {
        use std::collections::HashMap;

        if self.packages.is_empty() {
            return Vec::new();
        }

        // Assign levels using a simple algorithm:
        // Level 0: packages with no dependencies
        // Level N: packages whose maximum dependency level is N-1
        let mut levels: Vec<PublishLevel> = Vec::new();
        let mut pkg_level: HashMap<String, usize> = HashMap::new();

        for pkg in &self.packages {
            let deps = self.dependencies.get(&pkg.name).cloned().unwrap_or_default();
            
            // Find the maximum level of all dependencies
            let max_dep_level = deps
                .iter()
                .filter_map(|dep| pkg_level.get(dep).copied())
                .max()
                .unwrap_or(0);

            // This package goes to the next level
            let level = max_dep_level + 1;
            pkg_level.insert(pkg.name.clone(), level);

            // Ensure we have enough levels
            while levels.len() < level {
                levels.push(PublishLevel {
                    level: levels.len(),
                    packages: Vec::new(),
                });
            }

            levels[level - 1].packages.push(pkg.clone());
        }

        levels
    }
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
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["a", "b", "c", "d", "zeta", "alpha", "npdep"]
resolver = "2"
"#,
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
        assert!(names.contains(&"npdep".to_string()));
        assert!(!names.contains(&"c".to_string()));
        assert!(!names.contains(&"d".to_string()));

        let a_idx = names.iter().position(|n| n == "a").expect("a present");
        let b_idx = names.iter().position(|n| n == "b").expect("b present");
        assert!(a_idx < b_idx);
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
}
