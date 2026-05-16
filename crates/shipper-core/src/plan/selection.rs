use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context, Result};
use cargo_metadata::PackageId;

pub(super) fn resolve_included_packages(
    selected_packages: Option<&[String]>,
    publishable: &BTreeSet<PackageId>,
    deps_of: &BTreeMap<PackageId, BTreeSet<PackageId>>,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> Result<BTreeSet<PackageId>> {
    let Some(selected) = selected_packages else {
        return Ok(publishable.clone());
    };

    let name_to_id = publishable_package_names(publishable, pkg_map)?;
    let mut queue = VecDeque::new();
    let mut included = BTreeSet::new();

    for name in selected {
        let id = name_to_id
            .get(name)
            .with_context(|| format!("selected package not found or not publishable: {name}"))?
            .clone();
        if included.insert(id.clone()) {
            queue.push_back(id);
        }
    }

    // Include internal dependencies transitively.
    while let Some(id) = queue.pop_front() {
        if let Some(deps) = deps_of.get(&id) {
            for dep in deps {
                if included.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    Ok(included)
}

fn publishable_package_names(
    publishable: &BTreeSet<PackageId>,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> Result<BTreeMap<String, PackageId>> {
    let mut name_to_id = BTreeMap::new();
    for id in publishable {
        let pkg = pkg_map
            .get(id)
            .context("workspace package missing from metadata")?;
        name_to_id.insert(pkg.name.to_string(), id.clone());
    }
    Ok(name_to_id)
}
