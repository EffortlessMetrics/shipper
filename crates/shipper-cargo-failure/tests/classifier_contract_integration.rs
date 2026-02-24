use shipper_cargo_failure::{CargoFailureClass, classify_publish_failure};

#[test]
fn classifies_common_registry_throttling_errors_as_retryable() {
    let outcome = classify_publish_failure("received HTTP 503 from index", "");
    assert_eq!(outcome.class, CargoFailureClass::Retryable);
}

#[test]
fn classifies_manifest_validation_errors_as_permanent() {
    let outcome = classify_publish_failure("", "error: failed to parse manifest at Cargo.toml");
    assert_eq!(outcome.class, CargoFailureClass::Permanent);
}

#[test]
fn unknown_output_stays_ambiguous() {
    let outcome = classify_publish_failure("tool exited with status 101", "see logs");
    assert_eq!(outcome.class, CargoFailureClass::Ambiguous);
}
