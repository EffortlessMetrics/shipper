use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, PackageId};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::types::{PlannedPackage, ReleasePlan, ReleaseSpec};

#[derive(Debug, Clone)]
pub struct PlannedWorkspace {
    pub workspace_root: PathBuf,
    pub plan: ReleasePlan,
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

    // Workspace publishable set (restricted by `[package] publish` where possible).
    let publishable: BTreeSet<PackageId> = workspace_ids
        .iter()
        .filter(|id| {
            if let Some(pkg) = pkg_map.get(*id) {
                publish_allowed(pkg, &spec.registry.name)
            } else {
                false
            }
        })
        .cloned()
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

            let is_relevant = dep.dep_kinds.iter().any(|k| matches!(k.kind, DependencyKind::Normal | DependencyKind::Build));
            if !is_relevant {
                continue;
            }

            deps_of.entry(node.id.clone()).or_default().insert(dep.pkg.clone());
            dependents_of.entry(dep.pkg.clone()).or_default().insert(node.id.clone());
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
            name_to_id.insert(pkg.name.clone(), id.clone());
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
                name: pkg.name.clone(),
                version: pkg.version.to_string(),
                manifest_path: pkg.manifest_path.clone().into_std_path_buf(),
            }
        })
        .collect();

    let plan_id = compute_plan_id(&spec.registry.api_base, &packages);

    Ok(PlannedWorkspace {
        workspace_root,
        plan: ReleasePlan {
            plan_id,
            created_at: Utc::now(),
            registry: spec.registry.clone(),
            packages,
        },
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
            let name = pkg_map.get(id).map(|p| p.name.clone()).unwrap_or_default();
            ready.insert((name, id.clone()));
        }
    }

    let mut out: Vec<PackageId> = Vec::with_capacity(included.len());

    while let Some((_, id)) = ready.iter().next().cloned() {
        ready.remove(&(pkg_map.get(&id).unwrap().name.clone(), id.clone()));
        out.push(id.clone());

        if let Some(deps) = dependents_of.get(&id) {
            for dep in deps {
                if !included.contains(dep) {
                    continue;
                }
                if let Some(d) = indegree.get_mut(dep) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        let name = pkg_map.get(dep).unwrap().name.clone();
                        ready.insert((name, dep.clone()));
                    }
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
