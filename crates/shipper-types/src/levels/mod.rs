//! Dependency-level grouping for ordered publish plans.
//!
//! This module implements the "what can run in parallel" logic used by
//! [`crate::ReleasePlan::group_by_levels`]. It was previously the standalone
//! `shipper-levels` crate and was absorbed into `shipper-types` because
//! `ReleasePlan` (which lives here) is its primary consumer.

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

    #[test]
    fn single_package_no_deps() {
        let packages = vec!["solo".to_string()];
        let dependencies = deps(&[("solo", &[])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].level, 0);
        assert_eq!(names(&levels), vec![vec!["solo".to_string()]]);
    }

    #[test]
    fn single_package_missing_from_dependency_map() {
        let packages = vec!["solo".to_string()];
        let dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 1);
        assert_eq!(names(&levels), vec![vec!["solo".to_string()]]);
    }

    #[test]
    fn all_independent_packages_grouped_in_one_level() {
        let packages = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "gamma".to_string(),
            "delta".to_string(),
        ];
        let dependencies = deps(&[
            ("alpha", &[]),
            ("beta", &[]),
            ("gamma", &[]),
            ("delta", &[]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 1);
        assert_eq!(
            names(&levels),
            vec![vec![
                "alpha".to_string(),
                "beta".to_string(),
                "gamma".to_string(),
                "delta".to_string(),
            ]]
        );
    }

    #[test]
    fn level_numbers_are_sequential_from_zero() {
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let dependencies = deps(&[("a", &[]), ("b", &["a"]), ("c", &["b"]), ("d", &["c"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        for (i, level) in levels.iter().enumerate() {
            assert_eq!(level.level, i, "level index mismatch at position {i}");
        }
    }

    #[test]
    fn within_level_order_follows_input_order() {
        // z comes before a in input, both independent; level should preserve input order
        let packages = vec!["z".to_string(), "a".to_string(), "m".to_string()];
        let dependencies = deps(&[("z", &[]), ("a", &[]), ("m", &[])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 1);
        assert_eq!(
            names(&levels),
            vec![vec!["z".to_string(), "a".to_string(), "m".to_string()]]
        );
    }

    #[test]
    fn duplicate_packages_in_input_are_deduplicated() {
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
            "b".to_string(),
        ];
        let dependencies = deps(&[("a", &[]), ("b", &["a"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.into_iter().flat_map(|l| l.packages).collect();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn self_dependency_is_treated_as_cycle_fallback() {
        let packages = vec!["x".to_string()];
        let dependencies = deps(&[("x", &["x"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.into_iter().flat_map(|l| l.packages).collect();
        assert_eq!(flat, vec!["x".to_string()]);
    }

    #[test]
    fn three_way_cycle_drains_deterministically() {
        let packages = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let dependencies = deps(&[("a", &["c"]), ("b", &["a"]), ("c", &["b"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
        // All packages must appear exactly once
        assert_eq!(flat.len(), 3);
        let unique: BTreeSet<String> = flat.into_iter().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn diamond_dependency_pattern() {
        //     root
        //    /    \
        //  left   right
        //    \    /
        //     leaf
        let packages = vec![
            "root".to_string(),
            "left".to_string(),
            "right".to_string(),
            "leaf".to_string(),
        ];
        let dependencies = deps(&[
            ("root", &[]),
            ("left", &["root"]),
            ("right", &["root"]),
            ("leaf", &["left", "right"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 3);
        assert_eq!(names(&levels)[0], vec!["root".to_string()]);
        assert_eq!(
            names(&levels)[1],
            vec!["left".to_string(), "right".to_string()]
        );
        assert_eq!(names(&levels)[2], vec!["leaf".to_string()]);
    }

    #[test]
    fn wide_fan_out_from_single_root() {
        let mut packages = vec!["root".to_string()];
        let leaf_names: Vec<String> = (0..5).map(|i| format!("leaf-{i}")).collect();
        for name in &leaf_names {
            packages.push(name.clone());
        }

        let dependencies = {
            let mut m = BTreeMap::new();
            m.insert("root".to_string(), vec![]);
            for name in &leaf_names {
                m.insert(name.clone(), vec!["root".to_string()]);
            }
            m
        };

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].packages, vec!["root".to_string()]);
        assert_eq!(levels[1].packages.len(), 5);
    }

    #[test]
    fn deep_linear_chain() {
        let packages: Vec<String> = (0..8).map(|i| format!("pkg-{i}")).collect();
        let mut dep_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        dep_map.insert(packages[0].clone(), vec![]);
        for i in 1..packages.len() {
            dep_map.insert(packages[i].clone(), vec![packages[i - 1].clone()]);
        }

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dep_map);
        assert_eq!(levels.len(), 8);
        for (i, level) in levels.iter().enumerate() {
            assert_eq!(level.packages.len(), 1);
            assert_eq!(level.packages[0], format!("pkg-{i}"));
        }
    }

    #[test]
    fn partial_cycle_with_acyclic_tail() {
        // a -> b -> a (cycle), c depends on b (acyclic tail)
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let dependencies = deps(&[("a", &["b"]), ("b", &["a"]), ("c", &["b"]), ("d", &[])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
        let unique: BTreeSet<String> = flat.iter().cloned().collect();
        assert_eq!(flat.len(), 4);
        assert_eq!(unique.len(), 4);
    }

    #[test]
    fn works_with_custom_type() {
        #[derive(Clone, Debug, PartialEq)]
        struct Crate {
            name: String,
            version: u32,
        }

        let packages = vec![
            Crate {
                name: "core".into(),
                version: 1,
            },
            Crate {
                name: "app".into(),
                version: 2,
            },
        ];
        let dependencies = deps(&[("core", &[]), ("app", &["core"])]);

        let levels = group_packages_by_levels(&packages, |c| &c.name, &dependencies);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].packages[0].version, 1);
        assert_eq!(levels[1].packages[0].version, 2);
    }

    #[test]
    fn deps_on_packages_not_in_plan_are_ignored_for_leveling() {
        let packages = vec!["a".to_string(), "b".to_string()];
        let dependencies = deps(&[("a", &["external1", "external2"]), ("b", &["external3"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        // Both should be at level 0 since their deps aren't in the plan
        assert_eq!(levels.len(), 1);
        assert_eq!(names(&levels), vec![vec!["a".to_string(), "b".to_string()]]);
    }

    #[test]
    fn mixed_internal_and_external_deps() {
        let packages = vec!["core".to_string(), "mid".to_string(), "top".to_string()];
        let dependencies = deps(&[
            ("core", &["serde"]),
            ("mid", &["core", "tokio"]),
            ("top", &["mid", "anyhow", "serde"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 3);
        assert_eq!(names(&levels)[0], vec!["core".to_string()]);
        assert_eq!(names(&levels)[1], vec!["mid".to_string()]);
        assert_eq!(names(&levels)[2], vec!["top".to_string()]);
    }

    #[test]
    fn multiple_roots_with_shared_child() {
        let packages = vec!["r1".to_string(), "r2".to_string(), "child".to_string()];
        let dependencies = deps(&[("r1", &[]), ("r2", &[]), ("child", &["r1", "r2"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 2);
        assert_eq!(
            names(&levels),
            vec![
                vec!["r1".to_string(), "r2".to_string()],
                vec!["child".to_string()],
            ]
        );
    }

    #[test]
    fn disconnected_components_are_grouped_by_level() {
        // Two independent subgraphs: a->b and c->d
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let dependencies = deps(&[("a", &[]), ("b", &["a"]), ("c", &[]), ("d", &["c"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 2);
        assert_eq!(
            names(&levels),
            vec![
                vec!["a".to_string(), "c".to_string()],
                vec!["b".to_string(), "d".to_string()],
            ]
        );
    }

    #[test]
    fn disconnected_components_different_depths() {
        // Subgraph 1: a (depth 0), Subgraph 2: x->y->z (depth 2)
        let packages = vec![
            "a".to_string(),
            "x".to_string(),
            "y".to_string(),
            "z".to_string(),
        ];
        let dependencies = deps(&[("a", &[]), ("x", &[]), ("y", &["x"]), ("z", &["y"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 3);
        assert_eq!(names(&levels)[0], vec!["a".to_string(), "x".to_string()]);
        assert_eq!(names(&levels)[1], vec!["y".to_string()]);
        assert_eq!(names(&levels)[2], vec!["z".to_string()]);
    }

    #[test]
    fn two_node_cycle() {
        let packages = vec!["a".to_string(), "b".to_string()];
        let dependencies = deps(&[("a", &["b"]), ("b", &["a"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
        assert_eq!(flat.len(), 2);
        let unique: BTreeSet<String> = flat.into_iter().collect();
        assert_eq!(unique.len(), 2);
    }

    #[test]
    fn four_way_cycle_drains_all() {
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let dependencies = deps(&[("a", &["d"]), ("b", &["a"]), ("c", &["b"]), ("d", &["c"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
        assert_eq!(flat.len(), 4);
        let unique: BTreeSet<String> = flat.into_iter().collect();
        assert_eq!(unique.len(), 4);
    }

    #[test]
    fn cycle_with_disconnected_acyclic_component() {
        // a<->b (cycle), c->d (acyclic, disconnected)
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let dependencies = deps(&[("a", &["b"]), ("b", &["a"]), ("c", &[]), ("d", &["c"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
        assert_eq!(flat.len(), 4);
        // c must appear before d in the flattened output
        let pos_c = flat.iter().position(|n| n == "c").unwrap();
        let pos_d = flat.iter().position(|n| n == "d").unwrap();
        assert!(pos_c < pos_d);
        let unique: BTreeSet<String> = flat.into_iter().collect();
        assert_eq!(unique.len(), 4);
    }

    #[test]
    fn inverted_tree_fan_in() {
        // Multiple leaves converge on a single root
        let packages = vec![
            "leaf1".to_string(),
            "leaf2".to_string(),
            "leaf3".to_string(),
            "trunk".to_string(),
        ];
        let dependencies = deps(&[
            ("leaf1", &[]),
            ("leaf2", &[]),
            ("leaf3", &[]),
            ("trunk", &["leaf1", "leaf2", "leaf3"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].packages.len(), 3);
        assert_eq!(levels[1].packages, vec!["trunk".to_string()]);
    }

    #[test]
    fn w_shaped_dependency() {
        //  a   b   c
        //   \ / \ /
        //    d   e
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        let dependencies = deps(&[
            ("a", &[]),
            ("b", &[]),
            ("c", &[]),
            ("d", &["a", "b"]),
            ("e", &["b", "c"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 2);
        assert_eq!(
            names(&levels)[0],
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(names(&levels)[1], vec!["d".to_string(), "e".to_string()]);
    }

    #[test]
    fn level_zero_packages_have_no_internal_deps() {
        let packages = vec![
            "core".to_string(),
            "utils".to_string(),
            "api".to_string(),
            "app".to_string(),
        ];
        let dependencies = deps(&[
            ("core", &[]),
            ("utils", &["core"]),
            ("api", &["core"]),
            ("app", &["utils", "api"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let package_set: BTreeSet<String> = packages.iter().cloned().collect();
        for pkg in &levels[0].packages {
            let pkg_deps = dependencies.get(pkg).cloned().unwrap_or_default();
            let internal_deps: Vec<_> = pkg_deps
                .iter()
                .filter(|d| package_set.contains(*d))
                .collect();
            assert!(
                internal_deps.is_empty(),
                "level 0 package {pkg} has internal deps: {internal_deps:?}"
            );
        }
    }

    #[test]
    fn level_assignment_multi_layer_exact() {
        //  a (0)
        //  |
        //  b (1)
        //  |
        //  c (2) depends on a and b => level 2
        //  |
        //  d (3)
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let dependencies = deps(&[("a", &[]), ("b", &["a"]), ("c", &["a", "b"]), ("d", &["c"])]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 4);
        assert_eq!(levels[0].packages, vec!["a".to_string()]);
        assert_eq!(levels[0].level, 0);
        assert_eq!(levels[1].packages, vec!["b".to_string()]);
        assert_eq!(levels[1].level, 1);
        assert_eq!(levels[2].packages, vec!["c".to_string()]);
        assert_eq!(levels[2].level, 2);
        assert_eq!(levels[3].packages, vec!["d".to_string()]);
        assert_eq!(levels[3].level, 3);
    }

    #[test]
    fn large_graph_50_nodes_wide() {
        let mut packages = vec!["root".to_string()];
        let leaves: Vec<String> = (0..50).map(|i| format!("leaf-{i}")).collect();
        packages.extend(leaves.clone());

        let mut dep_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        dep_map.insert("root".to_string(), vec![]);
        for name in &leaves {
            dep_map.insert(name.clone(), vec!["root".to_string()]);
        }

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dep_map);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].packages, vec!["root".to_string()]);
        assert_eq!(levels[1].packages.len(), 50);
    }

    #[test]
    fn large_graph_layered_60_nodes() {
        // 6 layers of 10 nodes each; each layer depends on all nodes in previous layer
        let mut packages = Vec::new();
        let mut dep_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut prev_layer: Vec<String> = Vec::new();

        for layer in 0..6 {
            let mut current_layer = Vec::new();
            for i in 0..10 {
                let name = format!("l{layer}-n{i}");
                packages.push(name.clone());
                dep_map.insert(name.clone(), prev_layer.clone());
                current_layer.push(name);
            }
            prev_layer = current_layer;
        }

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dep_map);
        assert_eq!(levels.len(), 6);
        for level in &levels {
            assert_eq!(level.packages.len(), 10);
        }
        let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
        assert_eq!(flat.len(), 60);
    }

    #[test]
    fn large_linear_chain_50_nodes() {
        let packages: Vec<String> = (0..50).map(|i| format!("pkg-{i:03}")).collect();
        let mut dep_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        dep_map.insert(packages[0].clone(), vec![]);
        for i in 1..50 {
            dep_map.insert(packages[i].clone(), vec![packages[i - 1].clone()]);
        }

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dep_map);
        assert_eq!(levels.len(), 50);
        for (i, level) in levels.iter().enumerate() {
            assert_eq!(level.packages.len(), 1);
            assert_eq!(level.level, i);
        }
    }

    #[test]
    fn stability_repeated_runs_produce_identical_output() {
        let packages = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "gamma".to_string(),
            "delta".to_string(),
            "epsilon".to_string(),
        ];
        let dependencies = deps(&[
            ("alpha", &[]),
            ("beta", &["alpha"]),
            ("gamma", &["alpha"]),
            ("delta", &["beta", "gamma"]),
            ("epsilon", &["delta"]),
        ]);

        let reference = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        for _ in 0..100 {
            let result = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
            assert_eq!(result, reference, "non-deterministic output detected");
        }
    }

    #[test]
    fn double_diamond() {
        //     a
        //    / \
        //   b   c
        //    \ /
        //     d
        //    / \
        //   e   f
        //    \ /
        //     g
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "f".to_string(),
            "g".to_string(),
        ];
        let dependencies = deps(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["a"]),
            ("d", &["b", "c"]),
            ("e", &["d"]),
            ("f", &["d"]),
            ("g", &["e", "f"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 5);
        assert_eq!(names(&levels)[0], vec!["a".to_string()]);
        assert_eq!(names(&levels)[1], vec!["b".to_string(), "c".to_string()]);
        assert_eq!(names(&levels)[2], vec!["d".to_string()]);
        assert_eq!(names(&levels)[3], vec!["e".to_string(), "f".to_string()]);
        assert_eq!(names(&levels)[4], vec!["g".to_string()]);
    }

    #[test]
    fn package_with_empty_dep_list_in_map() {
        let packages = vec!["a".to_string(), "b".to_string()];
        let mut dep_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        dep_map.insert("a".to_string(), vec![]);
        dep_map.insert("b".to_string(), vec![]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dep_map);
        assert_eq!(levels.len(), 1);
        assert_eq!(names(&levels), vec![vec!["a".to_string(), "b".to_string()]]);
    }

    #[test]
    fn skip_level_impossible_with_correct_assignment() {
        // Verify that if c depends on a (skipping b's level), c is at level 1 not 2
        let packages = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let dependencies = deps(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["a"]), // c depends on a directly, not b
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 2);
        assert_eq!(names(&levels)[0], vec!["a".to_string()]);
        // b and c should be in the same level since both only depend on a
        assert_eq!(names(&levels)[1], vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn every_package_appears_exactly_once() {
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        let dependencies = deps(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["a"]),
            ("d", &["b", "c"]),
            ("e", &["d"]),
        ]);

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
        assert_eq!(flat.len(), packages.len());
        let unique: BTreeSet<String> = flat.into_iter().collect();
        assert_eq!(unique.len(), packages.len());
    }

    #[test]
    fn three_disconnected_singletons() {
        let packages = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        let dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();

        let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].level, 0);
        assert_eq!(levels[0].packages.len(), 3);
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

        #[test]
        fn level_indices_are_sequential(
            (names, dependencies) in dag_case(),
        ) {
            let levels = group_packages_by_levels(&names, |name| name.as_str(), &dependencies);
            for (i, level) in levels.iter().enumerate() {
                prop_assert_eq!(level.level, i);
            }
        }

        #[test]
        fn no_empty_levels_are_produced(
            (names, dependencies) in arbitrary_graph_case(),
        ) {
            let levels = group_packages_by_levels(&names, |name| name.as_str(), &dependencies);
            for level in &levels {
                prop_assert!(!level.packages.is_empty());
            }
        }

        #[test]
        fn deterministic_output_for_same_input(
            (names, dependencies) in dag_case(),
        ) {
            let levels1 = group_packages_by_levels(&names, |name| name.as_str(), &dependencies);
            let levels2 = group_packages_by_levels(&names, |name| name.as_str(), &dependencies);
            prop_assert_eq!(levels1, levels2);
        }
    }
}

#[cfg(test)]
mod proptests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use proptest::prelude::*;

    use super::*;

    fn dag_strategy() -> impl Strategy<Value = (Vec<String>, BTreeMap<String, Vec<String>>)> {
        (1usize..12).prop_flat_map(|n| {
            prop::collection::vec(any::<bool>(), n * n).prop_map(move |bits| {
                let names: Vec<String> = (0..n).map(|i| format!("crate-{i}")).collect();
                let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for i in 0..n {
                    let mut d = Vec::new();
                    for j in 0..i {
                        if bits[i * n + j] {
                            d.push(names[j].clone());
                        }
                    }
                    deps.insert(names[i].clone(), d);
                }
                (names, deps)
            })
        })
    }

    fn cyclic_strategy() -> impl Strategy<Value = (Vec<String>, BTreeMap<String, Vec<String>>)> {
        (2usize..10).prop_flat_map(|n| {
            prop::collection::vec(any::<bool>(), n * n).prop_map(move |bits| {
                let names: Vec<String> = (0..n).map(|i| format!("crate-{i}")).collect();
                let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for i in 0..n {
                    let mut d = Vec::new();
                    for j in 0..n {
                        if i != j && bits[i * n + j] {
                            d.push(names[j].clone());
                        }
                    }
                    deps.insert(names[i].clone(), d);
                }
                (names, deps)
            })
        })
    }

    fn dag_with_duplicates() -> impl Strategy<Value = (Vec<String>, BTreeMap<String, Vec<String>>)>
    {
        dag_strategy().prop_flat_map(|(names, deps)| {
            let n = names.len();
            prop::collection::vec(0..n, 0..n).prop_map(move |extra_indices| {
                let mut extended = names.clone();
                for idx in extra_indices {
                    extended.push(names[idx].clone());
                }
                (extended, deps.clone())
            })
        })
    }

    proptest! {
        #[test]
        fn level_count_bounded_by_package_count(
            (names, deps) in dag_strategy(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            prop_assert!(levels.len() <= names.len());
        }

        #[test]
        fn within_level_order_preserves_input_order(
            (names, deps) in dag_strategy(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            let position: HashMap<String, usize> = names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), i))
                .collect();
            for level in &levels {
                for pair in level.packages.windows(2) {
                    let pos_a = position.get(&pair[0]).unwrap();
                    let pos_b = position.get(&pair[1]).unwrap();
                    prop_assert!(pos_a < pos_b,
                        "within-level order violated: {} (pos {}) before {} (pos {})",
                        pair[0], pos_a, pair[1], pos_b);
                }
            }
        }

        #[test]
        fn all_independent_packages_land_at_level_zero(
            names in prop::collection::vec("[a-z]{1,8}", 1..10),
        ) {
            let unique: Vec<String> = {
                let mut seen = BTreeSet::new();
                names.into_iter().filter(|n| seen.insert(n.clone())).collect()
            };
            let deps: BTreeMap<String, Vec<String>> = unique
                .iter()
                .map(|n| (n.clone(), Vec::new()))
                .collect();
            let levels = group_packages_by_levels(&unique, |n| n.as_str(), &deps);
            prop_assert_eq!(levels.len(), 1);
            prop_assert_eq!(levels[0].level, 0);
            prop_assert_eq!(levels[0].packages.len(), unique.len());
        }

        #[test]
        fn transitive_dependency_ordering(
            (names, deps) in dag_strategy(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            let level_of: HashMap<String, usize> = levels
                .iter()
                .flat_map(|l| l.packages.iter().map(move |n| (n.clone(), l.level)))
                .collect();
            for (pkg, dep_list) in &deps {
                if let Some(&pkg_lvl) = level_of.get(pkg) {
                    for dep in dep_list {
                        if let Some(&dep_lvl) = level_of.get(dep) {
                            prop_assert!(dep_lvl < pkg_lvl,
                                "{} (level {}) depends on {} (level {})",
                                pkg, pkg_lvl, dep, dep_lvl);
                        }
                    }
                }
            }
        }

        #[test]
        fn duplicates_in_input_still_produce_unique_output(
            (names, deps) in dag_with_duplicates(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
            let unique: BTreeSet<String> = names.iter().cloned().collect();
            prop_assert_eq!(flat.len(), unique.len());
            let flat_set: BTreeSet<String> = flat.into_iter().collect();
            prop_assert_eq!(flat_set, unique);
        }

        #[test]
        fn publish_level_clone_equals_original(
            (names, deps) in dag_strategy(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            let cloned = levels.clone();
            prop_assert_eq!(&levels, &cloned);
        }

        #[test]
        fn cyclic_graphs_still_emit_all_packages_uniquely(
            (names, deps) in cyclic_strategy(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            let flat: Vec<String> = levels.iter().flat_map(|l| l.packages.clone()).collect();
            prop_assert_eq!(flat.len(), names.len());
            let unique: BTreeSet<String> = flat.into_iter().collect();
            prop_assert_eq!(unique.len(), names.len());
        }

        #[test]
        fn removing_leaf_preserves_earlier_levels(
            (names, deps) in dag_strategy(),
        ) {
            let depended_on: BTreeSet<String> = deps
                .values()
                .flat_map(|v| v.iter().cloned())
                .collect();
            let leaves: Vec<String> = names
                .iter()
                .filter(|n| !depended_on.contains(*n))
                .cloned()
                .collect();
            if let Some(leaf) = leaves.last() {
                let reduced_names: Vec<String> =
                    names.iter().filter(|n| *n != leaf).cloned().collect();
                let reduced_deps: BTreeMap<String, Vec<String>> = deps
                    .iter()
                    .filter(|(k, _)| *k != leaf)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let full_levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
                let reduced_levels =
                    group_packages_by_levels(&reduced_names, |n| n.as_str(), &reduced_deps);

                let full_map: HashMap<String, usize> = full_levels
                    .iter()
                    .flat_map(|l| l.packages.iter().map(move |n| (n.clone(), l.level)))
                    .collect();
                let reduced_map: HashMap<String, usize> = reduced_levels
                    .iter()
                    .flat_map(|l| l.packages.iter().map(move |n| (n.clone(), l.level)))
                    .collect();

                for (pkg, &lvl) in &reduced_map {
                    let &orig_lvl = full_map.get(pkg).unwrap();
                    prop_assert_eq!(lvl, orig_lvl,
                        "level of {} changed from {} to {} after removing leaf {}",
                        pkg, orig_lvl, lvl, leaf);
                }
            }
        }

        #[test]
        fn external_only_deps_yield_single_level(
            names in prop::collection::vec("[a-z]{1,6}", 1..8),
        ) {
            let unique: Vec<String> = {
                let mut seen = BTreeSet::new();
                names.into_iter().filter(|n| seen.insert(n.clone())).collect()
            };
            let deps: BTreeMap<String, Vec<String>> = unique
                .iter()
                .map(|n| (n.clone(), vec!["external-dep".to_string()]))
                .collect();
            let levels = group_packages_by_levels(&unique, |n| n.as_str(), &deps);
            prop_assert_eq!(levels.len(), 1);
        }

        #[test]
        fn dag_level_zero_has_no_internal_deps(
            (names, deps) in dag_strategy(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            let name_set: BTreeSet<String> = names.iter().cloned().collect();
            if !levels.is_empty() {
                for pkg in &levels[0].packages {
                    let pkg_deps = deps.get(pkg).cloned().unwrap_or_default();
                    let internal: Vec<_> = pkg_deps.iter().filter(|d| name_set.contains(*d)).collect();
                    prop_assert!(internal.is_empty(),
                        "level-0 package {} has internal deps: {:?}", pkg, internal);
                }
            }
        }

        #[test]
        fn dag_packages_assigned_to_minimal_possible_level(
            (names, deps) in dag_strategy(),
        ) {
            let levels = group_packages_by_levels(&names, |n| n.as_str(), &deps);
            let level_of: HashMap<String, usize> = levels
                .iter()
                .flat_map(|l| l.packages.iter().map(move |n| (n.clone(), l.level)))
                .collect();
            let name_set: BTreeSet<String> = names.iter().cloned().collect();
            for (pkg, pkg_deps) in &deps {
                if let Some(&pkg_lvl) = level_of.get(pkg) {
                    let max_dep_level = pkg_deps
                        .iter()
                        .filter(|d| name_set.contains(*d))
                        .filter_map(|d| level_of.get(d))
                        .max()
                        .copied();
                    let expected_min = max_dep_level.map_or(0, |l| l + 1);
                    prop_assert_eq!(pkg_lvl, expected_min,
                        "{} at level {} but min possible is {}", pkg, pkg_lvl, expected_min);
                }
            }
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use std::collections::BTreeMap;

    use super::*;
    use insta::assert_yaml_snapshot;

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

    fn level_summary(levels: &[PublishLevel<String>]) -> Vec<(usize, Vec<String>)> {
        levels
            .iter()
            .map(|l| (l.level, l.packages.clone()))
            .collect()
    }

    #[test]
    fn snapshot_diamond_dependency() {
        let packages = vec![
            "root".to_string(),
            "left".to_string(),
            "right".to_string(),
            "leaf".to_string(),
        ];
        let dependencies = deps(&[
            ("root", &[]),
            ("left", &["root"]),
            ("right", &["root"]),
            ("leaf", &["left", "right"]),
        ]);
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dependencies);
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_linear_chain() {
        let packages: Vec<String> = (0..5).map(|i| format!("pkg-{i}")).collect();
        let mut dep_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        dep_map.insert(packages[0].clone(), vec![]);
        for i in 1..packages.len() {
            dep_map.insert(packages[i].clone(), vec![packages[i - 1].clone()]);
        }
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dep_map);
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_all_independent() {
        let packages = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let dependencies = deps(&[("alpha", &[]), ("beta", &[]), ("gamma", &[])]);
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dependencies);
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_wide_fan_out() {
        let packages = vec![
            "root".to_string(),
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let dependencies = deps(&[
            ("root", &[]),
            ("a", &["root"]),
            ("b", &["root"]),
            ("c", &["root"]),
            ("d", &["root"]),
        ]);
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dependencies);
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_empty_plan() {
        let levels = group_packages_by_levels::<String, _>(&[], |n| n.as_str(), &BTreeMap::new());
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_disconnected_components() {
        let packages = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "gamma".to_string(),
            "delta".to_string(),
        ];
        let dependencies = deps(&[
            ("alpha", &[]),
            ("beta", &["alpha"]),
            ("gamma", &[]),
            ("delta", &["gamma"]),
        ]);
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dependencies);
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_deep_binary_tree() {
        //       a
        //      / \
        //     b   c
        //    / \
        //   d   e
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        let dependencies = deps(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["a"]),
            ("d", &["b"]),
            ("e", &["b"]),
        ]);
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dependencies);
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_cycle_fallback() {
        let packages = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let dependencies = deps(&[("a", &["c"]), ("b", &["a"]), ("c", &["b"])]);
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dependencies);
        assert_yaml_snapshot!(level_summary(&levels));
    }

    #[test]
    fn snapshot_double_diamond() {
        let packages = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "f".to_string(),
            "g".to_string(),
        ];
        let dependencies = deps(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["a"]),
            ("d", &["b", "c"]),
            ("e", &["d"]),
            ("f", &["d"]),
            ("g", &["e", "f"]),
        ]);
        let levels = group_packages_by_levels(&packages, |n| n.as_str(), &dependencies);
        assert_yaml_snapshot!(level_summary(&levels));
    }
}
