use shipper_core::types::GitContext;

#[test]
fn short_commit_public_reexport_preserves_ascii_and_utf8_fallback() {
    let ascii = GitContext {
        commit: Some("0123456789abcdef".to_owned()),
        ..GitContext::default()
    };
    assert_eq!(ascii.short_commit(), Some("0123456"));

    let unicode: GitContext = serde_json::from_str(r#"{"commit":"aaaaaaé"}"#)
        .expect("deserialize through the public core re-export");
    assert_eq!(unicode.short_commit(), Some("aaaaaaé"));

    let aligned = GitContext {
        commit: Some("aaa🦀-tail".to_owned()),
        ..GitContext::default()
    };
    assert_eq!(aligned.short_commit(), Some("aaa🦀"));
}
