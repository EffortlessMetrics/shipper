use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use cargo_metadata::PackageId;
use chrono::Utc;
use shipper_types::{PlannedWorkspace, ReleasePlan, ReleaseSpec};

use super::assembly::{compute_plan_id, dependency_map, planned_packages};
use super::graph::{build_dependency_graph, topo_sort, validate_publishable_dependencies};
use super::metadata::load_metadata;
use super::publishability::analyze_publishability;
use super::selection::resolve_included_packages;

pub(super) fn build_plan(spec: &ReleaseSpec) -> Result<PlannedWorkspace> {
    let metadata = load_metadata(&spec.manifest_path)?;
    let workspace_root = metadata.workspace_root.clone().into_std_path_buf();

    let pkg_map = metadata
        .packages
        .iter()
        .map(|p| (p.id.clone(), p))
        .collect::<BTreeMap<PackageId, &cargo_metadata::Package>>();
    let workspace_ids: BTreeSet<PackageId> = metadata.workspace_members.iter().cloned().collect();

    let publishability = analyze_publishability(&workspace_ids, &pkg_map, &spec.registry.name);
    let graph = build_dependency_graph(&workspace_ids, &publishability.publishable, &pkg_map)?;
    let included = resolve_included_packages(
        spec.selected_packages.as_deref(),
        &publishability.publishable,
        &graph.deps_of,
        &pkg_map,
    )?;

    validate_publishable_dependencies(&included, &graph, &pkg_map)?;

    let order = topo_sort(&included, &graph.deps_of, &graph.dependents_of, &pkg_map)?;
    let packages = planned_packages(&order, &pkg_map)?;
    let dependencies = dependency_map(&order, &included, &graph.deps_of, &pkg_map)?;
    let plan_id = compute_plan_id(&spec.registry.api_base, &packages);

    Ok(PlannedWorkspace {
        workspace_root,
        plan: ReleasePlan {
            plan_version: crate::state::execution_state::CURRENT_PLAN_VERSION.to_string(),
            plan_id,
            created_at: Utc::now(),
            registry: spec.registry.clone(),
            packages,
            dependencies,
        },
        skipped: publishability.skipped,
    })
}
