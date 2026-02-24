use shipper_cargo::redact_sensitive;

#[test]
fn cargo_crate_redact_sensitive_matches_output_sanitizer() {
    let input = "Authorization: Bearer shipper_secret_token\nCARGO_REGISTRY_TOKEN=abc123";
    assert_eq!(redact_sensitive(input), shipper_output_sanitizer::redact_sensitive(input));
}
