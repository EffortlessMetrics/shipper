#![no_main]

use std::collections::{BTreeMap, BTreeSet, HashMap};

use libfuzzer_sys::fuzz_target;
use shipper_levels::group_packages_by_levels;

fuzz_target!(|data: (u8, Vec<(u8, u8)>)| {
    let package_count = (data.0 as usize % 16) + 1;
    let packages: Vec<String> = (0..package_count).map(|i| format!("pkg-{i}")).collect();

    let mut dependencies: BTreeMap<String, Vec<String>> = packages
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();

    // Restrict edges to dep -> pkg where dep index < pkg index so the graph is a DAG.
    for (pkg_raw, dep_raw) in data.1.into_iter().take(256) {
        let pkg_idx = pkg_raw as usize % package_count;
        let dep_idx = dep_raw as usize % package_count;
        if dep_idx < pkg_idx {
            dependencies
                .get_mut(&packages[pkg_idx])
                .expect("package key exists")
                .push(packages[dep_idx].clone());
        }
    }

    let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);
    assert!(!levels.is_empty());

    for (idx, level) in levels.iter().enumerate() {
        assert_eq!(level.level, idx);
    }

    let mut flattened: Vec<String> = Vec::new();
    let mut level_by_package: HashMap<String, usize> = HashMap::new();
    for level in &levels {
        for package in &level.packages {
            flattened.push(package.clone());
            level_by_package.insert(package.clone(), level.level);
        }
    }
    assert_eq!(flattened.len(), package_count);

    let mut seen = BTreeSet::new();
    for package in &flattened {
        assert!(seen.insert(package.clone()));
    }

    for (pkg, deps) in &dependencies {
        let pkg_level = level_by_package
            .get(pkg)
            .copied()
            .expect("package exists");
        for dep in deps {
            let dep_level = level_by_package
                .get(dep)
                .copied()
                .expect("dependency exists");
            assert!(dep_level < pkg_level);
        }
    }
});
