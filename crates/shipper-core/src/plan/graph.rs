use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use cargo_metadata::{DependencyKind, PackageId};

pub(super) struct DependencyGraph {
    pub(super) deps_of: BTreeMap<PackageId, BTreeSet<PackageId>>,
    pub(super) dependents_of: BTreeMap<PackageId, BTreeSet<PackageId>>,
    non_publishable_workspace_deps: BTreeMap<PackageId, BTreeSet<PackageId>>,
}

pub(super) fn build_dependency_graph(
    workspace_ids: &BTreeSet<PackageId>,
    publishable: &BTreeSet<PackageId>,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> Result<DependencyGraph> {
    let pkg_dir_to_id = workspace_package_dirs(workspace_ids, pkg_map);

    let mut graph = DependencyGraph {
        deps_of: BTreeMap::new(),
        dependents_of: BTreeMap::new(),
        non_publishable_workspace_deps: BTreeMap::new(),
    };

    for id in workspace_ids {
        let pkg = pkg_map
            .get(id)
            .context("workspace package missing from metadata")?;
        for dep in &pkg.dependencies {
            if !matches!(dep.kind, DependencyKind::Normal | DependencyKind::Build) {
                continue;
            }
            let Some(dep_path) = dep.path.as_ref() else {
                continue;
            };
            let dep_dir = dep_path.clone().into_std_path_buf();
            let Some(dep_id) = pkg_dir_to_id.get(&dep_dir) else {
                continue;
            };

            record_workspace_dependency(&mut graph, publishable, id, dep_id);
        }
    }

    Ok(graph)
}

fn workspace_package_dirs(
    workspace_ids: &BTreeSet<PackageId>,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> BTreeMap<std::path::PathBuf, PackageId> {
    workspace_ids
        .iter()
        .filter_map(|id| {
            let pkg = pkg_map.get(id)?;
            let dir = pkg
                .manifest_path
                .parent()?
                .to_path_buf()
                .into_std_path_buf();
            Some((dir, id.clone()))
        })
        .collect()
}

fn record_workspace_dependency(
    graph: &mut DependencyGraph,
    publishable: &BTreeSet<PackageId>,
    id: &PackageId,
    dep_id: &PackageId,
) {
    if publishable.contains(id) && publishable.contains(dep_id) {
        graph
            .deps_of
            .entry(id.clone())
            .or_default()
            .insert(dep_id.clone());
        graph
            .dependents_of
            .entry(dep_id.clone())
            .or_default()
            .insert(id.clone());
    } else if publishable.contains(id) && !publishable.contains(dep_id) {
        graph
            .non_publishable_workspace_deps
            .entry(id.clone())
            .or_default()
            .insert(dep_id.clone());
    }
}

pub(super) fn validate_publishable_dependencies(
    included: &BTreeSet<PackageId>,
    graph: &DependencyGraph,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> Result<()> {
    for (id, dep_ids) in &graph.non_publishable_workspace_deps {
        if !included.contains(id) {
            continue;
        }
        let dep_id = dep_ids
            .iter()
            .next()
            .expect("non_publishable_workspace_deps entries are non-empty by construction");
        let pkg_name = pkg_map
            .get(id)
            .map(|p| p.name.as_str())
            .unwrap_or("unknown");
        let dep_name = pkg_map
            .get(dep_id)
            .map(|p| p.name.as_str())
            .unwrap_or("unknown");
        bail!(
            "publishable package '{}' depends on non-publishable workspace member '{}'",
            pkg_name,
            dep_name
        );
    }

    Ok(())
}

pub(super) fn topo_sort(
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
