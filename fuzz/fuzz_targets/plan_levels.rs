#![no_main]

use std::collections::BTreeMap;

use libfuzzer_sys::fuzz_target;
use shipper_types::levels::group_packages_by_levels;

// Fuzz the level assignment algorithm with arbitrary byte-driven dependency
// graphs, including cyclic and inconsistent topologies that the existing
// `release_levels` target (which only generates valid DAGs) cannot reach.
fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }

    // First byte controls package count (1..=32)
    let package_count = (data[0] as usize % 32) + 1;
    let packages: Vec<String> = (0..package_count).map(|i| format!("p{i}")).collect();

    let mut dependencies: BTreeMap<String, Vec<String>> = packages
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();

    // Remaining bytes encode edges: each pair (src, dst) adds a dependency.
    // Edges are NOT restricted to a DAG — cycles are intentionally allowed so
    // we exercise the fallback path in group_packages_by_levels.
    let edge_bytes = &data[1..];
    let mut i = 0;
    while i + 1 < edge_bytes.len() {
        let src_idx = edge_bytes[i] as usize % package_count;
        let dst_idx = edge_bytes[i + 1] as usize % package_count;
        if src_idx != dst_idx {
            dependencies
                .get_mut(&packages[src_idx])
                .expect("package key exists")
                .push(packages[dst_idx].clone());
        }
        i += 2;
    }

    let levels = group_packages_by_levels(&packages, |name| name.as_str(), &dependencies);

    // Every package must appear exactly once across all levels
    let total: usize = levels.iter().map(|l| l.packages.len()).sum();
    assert_eq!(total, package_count);

    // Levels must be numbered sequentially starting from 0
    for (idx, level) in levels.iter().enumerate() {
        assert_eq!(level.level, idx);
    }
});
