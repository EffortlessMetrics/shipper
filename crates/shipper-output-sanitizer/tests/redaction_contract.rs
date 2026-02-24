use shipper_output_sanitizer::{redact_sensitive, tail_lines};

#[test]
fn redaction_is_stable_for_common_credential_shapes() {
    let input = [
        "Authorization: Bearer super_secret_token",
        "token = \"hidden-token\"",
        "CARGO_REGISTRY_TOKEN=hidden",
        "CARGO_REGISTRIES_PRIVATE_REG_TOKEN=hidden",
        "normal output line",
    ]
    .join("\n");

    let out = redact_sensitive(&input);
    assert!(out.contains("Bearer [REDACTED]"));
    assert!(out.contains("token = [REDACTED]"));
    assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
    assert!(out.contains("CARGO_REGISTRIES_PRIVATE_REG_TOKEN=[REDACTED]"));
    assert!(out.contains("normal output line"));
    assert!(!out.contains("super_secret_token"));
    assert!(!out.contains("hidden-token"));
    assert!(!out.contains("hidden"));
}

#[test]
fn redaction_contract_matches_last_line_tail_behavior() {
    let input = "one\ntwo\nAuthorization: Bearer sensitive_token\nfour";
    assert_eq!(
        tail_lines(input, 2),
        "Authorization: Bearer [REDACTED]\nfour"
    );
    assert_eq!(
        tail_lines(input, 10),
        "one\ntwo\nAuthorization: Bearer [REDACTED]\nfour"
    );
}
