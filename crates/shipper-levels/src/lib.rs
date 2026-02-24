//! Dependency-level grouping for ordered publish plans.
//!
//! This crate extracts the "what can run in parallel" logic into a focused,
//! reusable component used by both monolithic and microcrate code paths.

use std::collections::{BTreeMap, BTreeSet};

/// A group of packages that can be processed in parallel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishLevel<T> {
    /// Zero-based level number.
    pub level: usize,
    /// Packages assigned to this level.
    pub packages: Vec<T>,
}

/// Group packages into dependency levels.
///
/// `ordered_packages` should be deterministic. Dependencies that are not part
/// of `ordered_packages` are ignored. If cyclic/inconsistent dependencies are
/// encountered, the function falls back to deterministic singleton progress so
/// every package still appears exactly once.
pub fn group_packages_by_levels<T, F>(
    ordered_packages: &[T],
    package_name: F,
    dependencies: &BTreeMap<String, Vec<String>>,
) -> Vec<PublishLevel<T>>
where
    T: Clone,
    F: Fn(&T) -> &str,
{
    let mut ordered_names: Vec<String> = Vec::new();
    let mut package_lookup: BTreeMap<String, T> = BTreeMap::new();

    for package in ordered_packages {
        let name = package_name(package).to_string();
        if package_lookup.contains_key(&name) {
            continue;
        }
        ordered_names.push(name.clone());
        package_lookup.insert(name, package.clone());
    }

    if ordered_names.is_empty() {
        return Vec::new();
    }

    let package_set: BTreeSet<String> = ordered_names.iter().cloned().collect();
    let mut indegree: BTreeMap<String, usize> = package_set
        .iter()
        .map(|name| (name.clone(), 0usize))
        .collect();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for name in &ordered_names {
        if let Some(deps) = dependencies.get(name) {
            for dep in deps {
                if !package_set.contains(dep) {
                    continue;
                }
                if let Some(degree) = indegree.get_mut(name) {
                    *degree += 1;
                }
                dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(name.clone());
            }
        }
    }

    let mut remaining: BTreeSet<String> = package_set;
    let mut levels: Vec<PublishLevel<T>> = Vec::new();

    while !remaining.is_empty() {
        let mut current: Vec<String> = ordered_names
            .iter()
            .filter(|name| {
                remaining.contains(*name) && indegree.get(*name).copied().unwrap_or(0) == 0
            })
            .cloned()
            .collect();

        // Cycles should be impossible for valid release plans. If present, keep
        // deterministic progress by draining one package at a time.
        if current.is_empty() {
            if let Some(name) = ordered_names
                .iter()
                .find(|name| remaining.contains(*name))
                .cloned()
            {
                current.push(name);
            } else {
                break;
            }
        }

        let packages = current
            .iter()
            .filter_map(|name| package_lookup.get(name).cloned())
            .collect();

        levels.push(PublishLevel {
            level: levels.len(),
            packages,
        });

        for name in current {
            remaining.remove(&name);
            if let Some(children) = dependents.get(&name) {
                for child in children {
                    if !remaining.contains(child) {
                        continue;
                    }
                    if let Some(degree) = indegree.get_mut(child) {
                        *degree = degree.saturating_sub(1);
                    }
                }
            }
        }
    }

    levels
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn deps(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(name, dep_list)| {
                (
                    (*name).to_string(),
                    dep_list.iter().map(|d| (*d).to_string()).collect(),
                )
            })
            .collect()
    }

    fn names(levels: &[PublishLevel<String>]) -> Vec<Vec<String>> {
        levels.iter().map(|l| l.packages.clone()).collect()
    }

    #[test]
    fn returns_empty_for_empty_input() {
        let levels =
            group_packages_by_levels::<String, _>(&[], |name| name.as_str(), &BTreeMap::new());
        assert!(levels.is_empty());
    }

    #[test]
    fn assigns_chain_to_strict_levels() {
        let packages = vec!["core".to_string(), "utils".to_string(), "app".to_string()];
        let dependencies = deps(&[("core", &[]), ("utils", &["core"]), ("app", &["utils"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(
            names(&levels),
            vec![
                vec!["core".to_string()],
                vec!["utils".to_string()],
                vec!["app".to_string()],
            ]
        );
    }

    #[test]
    fn assigns_independent_branches_to_same_level() {
        let packages = vec![
            "core".to_string(),
            "api".to_string(),
            "cli".to_string(),
            "app".to_string(),
        ];
        let dependencies = deps(&[
            ("core", &[]),
            ("api", &["core"]),
            ("cli", &["core"]),
            ("app", &["api", "cli"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(
            names(&levels),
            vec![
                vec!["core".to_string()],
                vec!["api".to_string(), "cli".to_string()],
                vec!["app".to_string()],
            ]
        );
    }

    #[test]
    fn ignores_dependencies_outside_the_plan() {
        let packages = vec!["core".to_string(), "app".to_string()];
        let dependencies = deps(&[("core", &[]), ("app", &["core", "serde", "tokio"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(
            names(&levels),
            vec![vec!["core".to_string()], vec!["app".to_string()]]
        );
    }

    #[test]
    fn falls_back_deterministically_when_cycle_is_present() {
        let packages = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let dependencies = deps(&[("a", &["b"]), ("b", &["a"]), ("c", &["b"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(
            names(&levels),
            vec![
                vec!["a".to_string()],
                vec!["b".to_string()],
                vec!["c".to_string()],
            ]
        );
    }
}

#[cfg(test)]
mod property_tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use proptest::prelude::*;

    use super::*;

    fn dag_case() -> impl Strategy<Value = (Vec<String>, BTreeMap<String, Vec<String>>)> {
        (1usize..10).prop_flat_map(|node_count| {
            prop::collection::vec(any::<bool>(), node_count * node_count).prop_map(move |bits| {
                let names: Vec<String> = (0..node_count).map(|i| format!("pkg-{i}")).collect();
                let mut dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();

                for i in 0..node_count {
                    let mut deps: Vec<String> = Vec::new();
                    for j in 0..i {
                        if bits[(i * node_count) + j] {
                            deps.push(names[j].clone());
                        }
                    }
                    dependencies.insert(names[i].clone(), deps);
                }

                (names, dependencies)
            })
        })
    }

    fn arbitrary_graph_case() -> impl Strategy<Value = (Vec<String>, BTreeMap<String, Vec<String>>)>
    {
        (1usize..10).prop_flat_map(|node_count| {
            prop::collection::vec(any::<bool>(), node_count * node_count).prop_map(move |bits| {
                let names: Vec<String> = (0..node_count).map(|i| format!("pkg-{i}")).collect();
                let mut dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();

                for i in 0..node_count {
                    let mut deps: Vec<String> = Vec::new();
                    for j in 0..node_count {
                        if i != j && bits[(i * node_count) + j] {
                            deps.push(names[j].clone());
                        }
                    }
                    dependencies.insert(names[i].clone(), deps);
                }

                (names, dependencies)
            })
        })
    }

    proptest! {
        #[test]
        fn dag_dependencies_always_point_to_earlier_levels(
            (names, dependencies) in dag_case(),
        ) {
            let levels = group_packages_by_levels(&names, |name| name.as_str(), &dependencies);

            let flattened: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
            prop_assert_eq!(flattened.len(), names.len());

            let mut seen: BTreeSet<String> = BTreeSet::new();
            for name in &flattened {
                prop_assert!(seen.insert(name.clone()));
            }

            let mut level_by_name: HashMap<String, usize> = HashMap::new();
            for (idx, level) in levels.iter().enumerate() {
                prop_assert_eq!(level.level, idx);
                for name in &level.packages {
                    level_by_name.insert(name.clone(), idx);
                }
            }

            for (pkg, deps) in &dependencies {
                if let Some(pkg_level) = level_by_name.get(pkg) {
                    for dep in deps {
                        if let Some(dep_level) = level_by_name.get(dep) {
                            prop_assert!(dep_level < pkg_level);
                        }
                    }
                }
            }
        }

        #[test]
        fn arbitrary_graphs_still_return_all_packages_once(
            (names, dependencies) in arbitrary_graph_case(),
        ) {
            let levels = group_packages_by_levels(&names, |name| name.as_str(), &dependencies);
            let flattened: Vec<String> = levels.into_iter().flat_map(|l| l.packages).collect();

            prop_assert_eq!(flattened.len(), names.len());

            let mut seen: BTreeSet<String> = BTreeSet::new();
            for name in &flattened {
                prop_assert!(seen.insert(name.clone()));
            }
        }
    }
}
