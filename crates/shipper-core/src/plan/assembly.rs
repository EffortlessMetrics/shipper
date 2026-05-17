use std::collections::{BTreeMap, BTreeSet};

use cargo_metadata::PackageId;
use sha2::{Digest, Sha256};
use shipper_types::PlannedPackage;

pub(super) fn planned_packages(
    order: &[PackageId],
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> Vec<PlannedPackage> {
    order
        .iter()
        .map(|id| {
            let pkg = pkg_map.get(id).expect("pkg exists");
            PlannedPackage {
                name: pkg.name.to_string(),
                version: pkg.version.to_string(),
                manifest_path: pkg.manifest_path.clone().into_std_path_buf(),
                regime: None,
            }
        })
        .collect()
}

pub(super) fn dependency_map(
    order: &[PackageId],
    included: &BTreeSet<PackageId>,
    deps_of: &BTreeMap<PackageId, BTreeSet<PackageId>>,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
) -> BTreeMap<String, Vec<String>> {
    let mut dependencies = BTreeMap::new();
    for id in order {
        let pkg = pkg_map.get(id).expect("pkg exists");
        let pkg_name = pkg.name.to_string();

        // Get all dependencies of this package that are in the plan.
        let dep_names = deps_of
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

    dependencies
}

pub(super) fn compute_plan_id(registry_api_base: &str, packages: &[PlannedPackage]) -> String {
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
