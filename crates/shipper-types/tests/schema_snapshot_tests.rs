use shipper_types::schema::{parse_schema_version, validate_schema_version};

// --- parse_schema_version snapshots ---

#[test]
fn snapshot_parse_valid_receipt_v2() {
    let result = parse_schema_version("shipper.receipt.v2");
    insta::assert_yaml_snapshot!(result.unwrap());
}

#[test]
fn snapshot_parse_valid_state_v1() {
    let result = parse_schema_version("shipper.state.v1");
    insta::assert_yaml_snapshot!(result.unwrap());
}

#[test]
fn snapshot_parse_valid_large_version() {
    let result = parse_schema_version("shipper.receipt.v999");
    insta::assert_yaml_snapshot!(result.unwrap());
}

#[test]
fn snapshot_parse_error_completely_invalid() {
    let err = parse_schema_version("invalid").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_parse_error_wrong_prefix() {
    let err = parse_schema_version("other.receipt.v2").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_parse_error_missing_v_prefix() {
    let err = parse_schema_version("shipper.receipt.2").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_parse_error_non_numeric_version() {
    let err = parse_schema_version("shipper.receipt.vx").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_parse_error_too_few_parts() {
    let err = parse_schema_version("shipper.v1").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_parse_error_too_many_parts() {
    let err = parse_schema_version("shipper.receipt.extra.v1").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_parse_error_empty_string() {
    let err = parse_schema_version("").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

// --- validate_schema_version snapshots ---

#[test]
fn snapshot_validate_exact_minimum() {
    let result = validate_schema_version("shipper.receipt.v1", "shipper.receipt.v1", "receipt");
    insta::assert_yaml_snapshot!(format!("{result:?}"));
}

#[test]
fn snapshot_validate_newer_than_minimum() {
    let result = validate_schema_version("shipper.receipt.v5", "shipper.receipt.v1", "receipt");
    insta::assert_yaml_snapshot!(format!("{result:?}"));
}

#[test]
fn snapshot_validate_error_too_old() {
    let err =
        validate_schema_version("shipper.receipt.v0", "shipper.receipt.v1", "receipt").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_validate_error_too_old_state() {
    let err = validate_schema_version("shipper.state.v1", "shipper.state.v3", "state").unwrap_err();
    insta::assert_yaml_snapshot!(err.to_string());
}

#[test]
fn snapshot_validate_error_invalid_version_format() {
    let err = validate_schema_version("bad-format", "shipper.receipt.v1", "receipt").unwrap_err();
    insta::assert_yaml_snapshot!(format!("{err:?}"));
}

#[test]
fn snapshot_validate_error_invalid_minimum_format() {
    let err = validate_schema_version("shipper.receipt.v2", "bad-minimum", "receipt").unwrap_err();
    insta::assert_yaml_snapshot!(format!("{err:?}"));
}

// --- version compatibility matrix ---

#[test]
fn snapshot_version_compatibility_matrix() {
    let cases = [
        ("shipper.receipt.v1", "shipper.receipt.v1"),
        ("shipper.receipt.v2", "shipper.receipt.v1"),
        ("shipper.receipt.v0", "shipper.receipt.v1"),
        ("shipper.state.v3", "shipper.state.v2"),
        ("shipper.state.v1", "shipper.state.v5"),
    ];

    let results: Vec<String> = cases
        .iter()
        .map(|(version, minimum)| {
            let result = validate_schema_version(version, minimum, "test");
            match result {
                Ok(()) => format!("{version} >= {minimum}: ok"),
                Err(e) => format!("{version} >= {minimum}: {e}"),
            }
        })
        .collect();

    insta::assert_yaml_snapshot!(results);
}
