#![no_main]

use std::fs;
use std::path::Path;

use libfuzzer_sys::fuzz_target;
use shipper::plan::build_plan;
use shipper_types::{Registry, ReleaseSpec};
use tempfile::tempdir;

// Fuzz that plan ordering is deterministic: building a plan twice from
// identical workspace manifests must produce identical package order and
// plan IDs.
fuzz_target!(|data: (u8, Vec<(Vec<u8>, Vec<u8>)>, Vec<(u8, u8)>)| {
    let (pkg_count_hint, raw_packages, raw_edges) = data;

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

        let mut seen_deps = std::collections::BTreeSet::new();
        let mut deps = String::new();
        for &(from_raw, to_raw) in raw_edges.iter().take(64) {
            let from_idx = from_raw as usize % pkg_count;
            let to_idx = to_raw as usize % pkg_count;
            if from_idx == i && to_idx < i && seen_deps.insert(to_idx) {
                let dep_name = &packages[to_idx].0;
                deps.push_str(&format!("{dep_name} = {{ path = \"../{dep_name}\" }}\n"));
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

    // Build the plan twice and assert identical results.
    let first = match build_plan(&spec) {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let second = match build_plan(&spec) {
        Ok(ws) => ws,
        Err(_) => panic!("second build_plan failed but first succeeded"),
    };

    assert_eq!(
        first.plan.plan_id, second.plan.plan_id,
        "plan_id must be deterministic"
    );
    assert_eq!(
        first.plan.packages.len(),
        second.plan.packages.len(),
        "package count must be deterministic"
    );
    for (a, b) in first.plan.packages.iter().zip(second.plan.packages.iter()) {
        assert_eq!(a.name, b.name, "package order must be deterministic");
        assert_eq!(a.version, b.version, "version must be deterministic");
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
