use shipper_cargo_failure::{CargoFailureClass, CargoFailureOutcome, classify_publish_failure};

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

#[test]
fn public_api_outcome_is_copy_and_eq() {
    let a: CargoFailureOutcome = classify_publish_failure("503", "");
    let b: CargoFailureOutcome = a;
    assert_eq!(a, b);
}

#[test]
fn public_api_class_is_copy() {
    let c = CargoFailureClass::Retryable;
    let d = c;
    assert_eq!(c, d);
}

#[test]
fn public_api_ambiguous_default_message_through_public_surface() {
    let o = classify_publish_failure("", "");
    assert_eq!(o.class, CargoFailureClass::Ambiguous);
    assert!(o.message.contains("ambiguous"));
}

#[test]
fn public_api_ambiguous_drives_reconciliation_path() {
    let o = classify_publish_failure("Uploading my-crate v0.1.0", "");
    assert_eq!(o.class, CargoFailureClass::Ambiguous);
}

#[test]
fn public_api_retryable_dominates_permanent_across_streams() {
    let o = classify_publish_failure("token is invalid", "503");
    assert_eq!(o.class, CargoFailureClass::Retryable);
}

#[test]
fn public_api_dep_resolution_is_permanent_not_ambiguous() {
    let o = classify_publish_failure(
        "error: failed to select a version for the requirement `x = \"^0.1\"`",
        "",
    );
    assert_eq!(o.class, CargoFailureClass::Permanent);
}

#[test]
fn public_api_no_matching_package_is_permanent() {
    let o = classify_publish_failure("error: no matching package named `nope` found", "");
    assert_eq!(o.class, CargoFailureClass::Permanent);
}
