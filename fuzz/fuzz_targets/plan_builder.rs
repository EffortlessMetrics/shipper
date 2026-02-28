#![no_main]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use libfuzzer_sys::fuzz_target;
use shipper_plan::build_plan;
use shipper_types::{Registry, ReleasePlan, ReleaseSpec};
use tempfile::tempdir;

fuzz_target!(|data: (u8, Vec<(Vec<u8>, Vec<u8>)>, Vec<(u8, u8)>, &[u8])| {
    let (pkg_count_hint, raw_packages, raw_edges, plan_json) = data;

    // --- ReleasePlan deserialization from arbitrary bytes ---
    if let Ok(json_str) = std::str::from_utf8(plan_json) {
        if let Ok(plan) = serde_json::from_str::<ReleasePlan>(json_str) {
            if let Ok(roundtripped) = serde_json::to_string(&plan) {
                if let Ok(parsed) = serde_json::from_str::<ReleasePlan>(&roundtripped) {
                    assert_eq!(plan.plan_id, parsed.plan_id);
                    assert_eq!(plan.packages.len(), parsed.packages.len());
                    for (a, b) in plan.packages.iter().zip(parsed.packages.iter()) {
                        assert_eq!(a.name, b.name);
                        assert_eq!(a.version, b.version);
                    }
                }
            }
        }
    }

    // --- Plan building with arbitrary workspace manifests and dependency graphs ---
    let pkg_count = ((pkg_count_hint as usize) % 6) + 1;
    let pkg_count = pkg_count.min(raw_packages.len());
    if pkg_count == 0 {
        return;
    }

    let packages: Vec<(String, String)> = raw_packages
        .iter()
        .take(pkg_count)
        .enumerate()
        .map(|(i, (name_bytes, ver_bytes))| {
            let name = make_crate_name(name_bytes, i);
            let version = make_version(ver_bytes);
            (name, version)
        })
        .collect();

    let td = match tempdir() {
        Ok(v) => v,
        Err(_) => return,
    };
    let root = td.path();

    let members: Vec<String> = packages.iter().map(|(n, _)| format!("\"{n}\"")).collect();
    let workspace_toml = format!(
        "[workspace]\nmembers = [{}]\nresolver = \"2\"\n",
        members.join(", ")
    );
    write_file(&root.join("Cargo.toml"), &workspace_toml);

    for (i, (name, version)) in packages.iter().enumerate() {
        let pkg_dir = root.join(name);
        let src_dir = pkg_dir.join("src");
        let _ = fs::create_dir_all(&src_dir);
        let _ = fs::write(src_dir.join("lib.rs"), "");

        // Restrict edges to dep_idx < pkg_idx so the graph is a DAG.
        let mut seen_deps = std::collections::BTreeSet::new();
        let mut deps = String::new();
        for &(from_raw, to_raw) in raw_edges.iter().take(64) {
            let from_idx = from_raw as usize % pkg_count;
            let to_idx = to_raw as usize % pkg_count;
            if from_idx == i && to_idx < i && seen_deps.insert(to_idx) {
                let dep_name = &packages[to_idx].0;
                deps.push_str(&format!(
                    "{dep_name} = {{ path = \"../{dep_name}\" }}\n"
                ));
            }
        }

        let manifest = format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
        );
        write_file(&pkg_dir.join("Cargo.toml"), &manifest);
    }

    let spec = ReleaseSpec {
        manifest_path: root.join("Cargo.toml"),
        registry: Registry::crates_io(),
        selected_packages: None,
    };

    // Exercises: load_metadata -> publish filtering -> topo_sort -> compute_plan_id
    if let Ok(ws) = build_plan(&spec) {
        assert!(!ws.plan.plan_id.is_empty());
        assert!(!ws.plan.packages.is_empty());

        // Every dependency must appear before its dependent in the plan.
        let pos: BTreeMap<String, usize> = ws
            .plan
            .packages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), i))
            .collect();
        for (pkg, deps) in &ws.plan.dependencies {
            if let Some(&pkg_pos) = pos.get(pkg) {
                for dep in deps {
                    if let Some(&dep_pos) = pos.get(dep) {
                        assert!(dep_pos < pkg_pos, "dep {dep} must come before {pkg}");
                    }
                }
            }
        }
    }
});

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, content);
}

fn make_crate_name(bytes: &[u8], index: usize) -> String {
    let base: String = bytes
        .iter()
        .take(6)
        .map(|b| (b'a' + (b % 26)) as char)
        .collect();
    if base.is_empty() {
        format!("crate{index}")
    } else {
        format!("c{base}{index}")
    }
}

fn make_version(bytes: &[u8]) -> String {
    let major = bytes.first().copied().unwrap_or(0) % 10;
    let minor = bytes.get(1).copied().unwrap_or(0) % 20;
    let patch = bytes.get(2).copied().unwrap_or(1) % 100;
    format!("{major}.{minor}.{patch}")
}
