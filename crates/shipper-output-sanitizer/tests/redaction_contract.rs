use shipper_output_sanitizer::{redact_sensitive, tail_lines};

#[test]
fn redaction_is_stable_for_common_credential_shapes() {
    let input = [
        format!("Authorization: Bearer {}", ["contract", "auth"].concat()),
        "token = \"hidden-token\"".to_string(),
        "CARGO_REGISTRY_TOKEN=hidden".to_string(),
        "CARGO_REGISTRIES_PRIVATE_REG_TOKEN=hidden".to_string(),
        "normal output line".to_string(),
    ]
    .join("\n");

    let out = redact_sensitive(&input);
    assert!(out.contains("Authorization: [REDACTED]"));
    assert!(out.contains(r#"token = "[REDACTED]""#));
    assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
    assert!(out.contains("CARGO_REGISTRIES_PRIVATE_REG_TOKEN=[REDACTED]"));
    assert!(out.contains("normal output line"));
    assert!(!out.contains("hidden-token"));
    assert!(!out.contains("hidden"));
}

#[test]
fn redaction_contract_covers_headers_and_structured_fields() {
    let input = [
        format!("Authorization: Bearer {}", ["contract", "bearer"].concat()),
        format!("Authorization: Basic {}", ["contract", "basic"].concat()),
        format!("Authorization: Digest {}", ["contract", "digest"].concat()),
        format!(
            "Proxy-Authorization: FutureScheme {}",
            ["contract", "proxy"].concat()
        ),
        r#"{"authorization":"Bearer structured-value","message":"Basic remains prose"}"#
            .to_string(),
    ]
    .join("\n");

    let out = redact_sensitive(&input);
    assert_eq!(
        out.lines()
            .filter(|line| line.contains("[REDACTED]"))
            .count(),
        5
    );
    assert!(out.contains(r#""message":"Basic remains prose""#));
    assert!(!out.contains("contract"));
    assert!(!out.contains("structured-value"));
}

#[test]
fn redaction_contract_matches_last_line_tail_behavior() {
    let input = "one\ntwo\nAuthorization: Bearer sensitive_token\nfour";
    assert_eq!(tail_lines(input, 2), "Authorization: [REDACTED]\nfour");
    assert_eq!(
        tail_lines(input, 10),
        "one\ntwo\nAuthorization: [REDACTED]\nfour"
    );
}
