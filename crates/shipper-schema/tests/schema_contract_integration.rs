use shipper_schema::{parse_schema_version, validate_schema_version};

#[test]
fn external_callers_can_validate_schema_compatibility() {
    let parsed = parse_schema_version("shipper.state.v3").expect("parse");
    assert_eq!(parsed, 3);

    validate_schema_version("shipper.state.v3", "shipper.state.v1", "schema")
        .expect("should be supported");
}

#[test]
fn external_callers_get_actionable_validation_errors() {
    let err = validate_schema_version("shipper.state.v0", "shipper.state.v1", "schema")
        .expect_err("must fail");
    assert!(err.to_string().contains("too old"));
}
