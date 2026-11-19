use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cargo_metadata::PackageId;

pub(super) fn close_over_dependencies(
    seed_ids: &BTreeSet<PackageId>,
    deps_of: &BTreeMap<PackageId, BTreeSet<PackageId>>,
) -> BTreeSet<PackageId> {
    let mut queue = VecDeque::from_iter(seed_ids.iter().cloned());
    let mut included = seed_ids.clone();

    while let Some(id) = queue.pop_front() {
        if let Some(deps) = deps_of.get(&id) {
            for dep in deps {
                if included.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    included
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> PackageId {
        PackageId {
            repr: name.to_string(),
        }
    }

    fn ids(names: &[&str]) -> BTreeSet<PackageId> {
        names.iter().map(|name| id(name)).collect()
    }

    fn graph(entries: &[(&str, &[&str])]) -> BTreeMap<PackageId, BTreeSet<PackageId>> {
        entries
            .iter()
            .map(|(package, dependencies)| (id(package), ids(dependencies)))
            .collect()
    }

    #[test]
    fn includes_transitive_dependencies() {
        let dependencies = graph(&[("facade", &["middle"]), ("middle", &["foundation"])]);

        assert_eq!(
            close_over_dependencies(&ids(&["facade"]), &dependencies),
            ids(&["facade", "middle", "foundation"])
        );
    }

    #[test]
    fn excludes_disconnected_components() {
        let dependencies = graph(&[
            ("facade", &["foundation"]),
            ("unrelated", &["other-foundation"]),
        ]);

        assert_eq!(
            close_over_dependencies(&ids(&["facade"]), &dependencies),
            ids(&["facade", "foundation"])
        );
    }

    #[test]
    fn multiple_seeds_share_dependencies_without_duplication() {
        let dependencies = graph(&[
            ("cli", &["core"]),
            ("facade", &["core"]),
            ("core", &["types"]),
        ]);

        assert_eq!(
            close_over_dependencies(&ids(&["facade", "cli"]), &dependencies),
            ids(&["facade", "cli", "core", "types"])
        );
    }

    #[test]
    fn cycles_terminate_and_include_each_package_once() {
        let dependencies = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);

        assert_eq!(
            close_over_dependencies(&ids(&["a"]), &dependencies),
            ids(&["a", "b", "c"])
        );
    }

    #[test]
    fn missing_graph_entries_leave_the_seed_unchanged() {
        assert_eq!(
            close_over_dependencies(&ids(&["leaf"]), &BTreeMap::new()),
            ids(&["leaf"])
        );
    }

    #[test]
    fn empty_selection_stays_empty_even_when_the_graph_has_edges() {
        let dependencies = graph(&[("facade", &["core"])]);

        assert!(close_over_dependencies(&BTreeSet::new(), &dependencies).is_empty());
    }
}
