use std::collections::{BTreeMap, BTreeSet};

use cargo_metadata::PackageId;
use shipper_types::SkippedPackage;

pub(super) struct Publishability {
    pub(super) publishable: BTreeSet<PackageId>,
    pub(super) skipped: Vec<SkippedPackage>,
}

pub(super) fn analyze_publishability(
    workspace_ids: &BTreeSet<PackageId>,
    pkg_map: &BTreeMap<PackageId, &cargo_metadata::Package>,
    registry_name: &str,
) -> Publishability {
    let mut skipped = Vec::new();

    let publishable = workspace_ids
        .iter()
        .filter_map(|id| {
            let pkg = pkg_map.get(id)?;
            if publish_allowed(pkg, registry_name) {
                Some(id.clone())
            } else {
                skipped.push(SkippedPackage {
                    name: pkg.name.to_string(),
                    version: pkg.version.to_string(),
                    reason: skip_reason(pkg),
                });
                None
            }
        })
        .collect();

    Publishability {
        publishable,
        skipped,
    }
}

fn skip_reason(pkg: &cargo_metadata::Package) -> String {
    match &pkg.publish {
        None => "publish not specified (default allowed)".to_string(),
        Some(list) if list.is_empty() => "publish = false".to_string(),
        Some(list) => format!("publish = {} (registry not in list)", list.join(", ")),
    }
}

pub(super) fn publish_allowed(pkg: &cargo_metadata::Package, registry_name: &str) -> bool {
    match &pkg.publish {
        None => true,
        Some(list) if list.is_empty() => false,
        Some(list) => {
            // Cargo uses `crates-io` as the default registry name.
            list.iter().any(|r| r == registry_name)
        }
    }
}
