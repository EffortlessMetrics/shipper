#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_sparse_index::{contains_version, sparse_index_path};

fuzz_target!(|data: (String, String, Vec<String>)| {
    let (crate_name, target, versions) = data;

    let path = sparse_index_path(&crate_name);
    assert_eq!(path, sparse_index_path(&crate_name));

    if !crate_name.is_empty() {
        assert!(path.ends_with(&crate_name.to_ascii_lowercase()));
    }

    let mut content_lines = versions
        .iter()
        .take(256)
        .map(|version| format!("{{\"vers\":\"{}\"}}", version))
        .collect::<Vec<_>>();
    content_lines.push("not-json".to_string());
    let content = content_lines.join("\n");

    let result = contains_version(&content, &target);
    assert_eq!(result, contains_version(&content, &target));

    let guaranteed = format!("{{\"vers\":\"{}\"}}", target);
    let guaranteed_content = format!("{content}\n{guaranteed}");
    assert!(contains_version(&guaranteed_content, &target));
});
