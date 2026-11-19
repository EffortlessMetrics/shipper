use shipper_sparse_index::{contains_version, sparse_index_path};

#[test]
fn sparse_index_path_matches_known_real_world_crates() {
    assert_eq!(sparse_index_path("serde"), "se/rd/serde");
    assert_eq!(sparse_index_path("tokio"), "to/ki/tokio");
    assert_eq!(sparse_index_path("clap"), "cl/ap/clap");
}

#[test]
fn sparse_index_path_derives_unicode_paths_without_registry_validation() {
    assert_eq!(sparse_index_path("é"), "1/é");
    assert_eq!(sparse_index_path("éß"), "2/éß");
    assert_eq!(sparse_index_path("åßç"), "3/å/åßç");
    assert_eq!(sparse_index_path("café"), "ca/fé/café");
}

#[test]
fn contains_version_handles_jsonl_contract() {
    let content = r#"{"name":"demo","vers":"0.1.0"}
{"name":"demo","vers":"0.2.0"}
{"name":"demo","vers":"1.0.0"}"#;

    assert!(contains_version(content, "0.2.0"));
    assert!(!contains_version(content, "2.0.0"));
}

#[test]
fn contains_version_ignores_non_json_and_unknown_shapes() {
    let content = r#"not-json
{"name":"demo"}
{"vers":"3.4.5"}"#;

    assert!(contains_version(content, "3.4.5"));
    assert!(!contains_version(content, "3.4.6"));
}
