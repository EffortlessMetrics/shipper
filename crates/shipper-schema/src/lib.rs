//! Schema version parsing and compatibility validation for shipper.

use anyhow::{Context, Result};

/// Parse schema version number from a string like `shipper.receipt.v2`.
///
/// # Examples
///
/// ```
/// use shipper_schema::parse_schema_version;
///
/// assert_eq!(parse_schema_version("shipper.receipt.v2").unwrap(), 2);
/// assert_eq!(parse_schema_version("shipper.state.v1").unwrap(), 1);
/// assert!(parse_schema_version("invalid").is_err());
/// ```
pub fn parse_schema_version(version: &str) -> Result<u32> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 || !parts[0].starts_with("shipper") || !parts[2].starts_with('v') {
        anyhow::bail!("invalid schema version format: {version}");
    }

    let version_part = &parts[2][1..];
    version_part
        .parse::<u32>()
        .with_context(|| format!("invalid version number in schema version: {version}"))
}

/// Validate that `version` is at least the minimum supported schema version.
///
/// The `label` value is used in error messages (for example: `receipt`, `schema`).
///
/// # Examples
///
/// ```
/// use shipper_schema::validate_schema_version;
///
/// // Accepted: version meets minimum
/// assert!(validate_schema_version("shipper.receipt.v2", "shipper.receipt.v1", "receipt").is_ok());
///
/// // Rejected: version is too old
/// assert!(validate_schema_version("shipper.receipt.v0", "shipper.receipt.v1", "receipt").is_err());
/// ```
pub fn validate_schema_version(version: &str, minimum_supported: &str, label: &str) -> Result<()> {
    let version_num = parse_schema_version(version)
        .with_context(|| format!("invalid {label} version format: {version}"))?;

    let minimum_num = parse_schema_version(minimum_supported)
        .with_context(|| format!("invalid minimum version format: {minimum_supported}"))?;

    if version_num < minimum_num {
        anyhow::bail!(
            "{label} version {version} is too old. Minimum supported version is {minimum_supported}"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_schema_version_extracts_numeric_suffix() {
        let parsed = parse_schema_version("shipper.receipt.v42").expect("parse");
        assert_eq!(parsed, 42);
    }

    #[test]
    fn parse_schema_version_rejects_invalid_prefix() {
        let err = parse_schema_version("other.receipt.v2").expect_err("must fail");
        assert!(err.to_string().contains("invalid schema version format"));
    }

    #[test]
    fn parse_schema_version_rejects_missing_v_prefix() {
        let err = parse_schema_version("shipper.receipt.2").expect_err("must fail");
        assert!(err.to_string().contains("invalid schema version format"));
    }

    #[test]
    fn parse_schema_version_rejects_non_numeric_suffix() {
        let err = parse_schema_version("shipper.receipt.vx").expect_err("must fail");
        assert!(err.to_string().contains("invalid version number"));
    }

    #[test]
    fn validate_schema_version_accepts_supported_versions() {
        validate_schema_version("shipper.receipt.v1", "shipper.receipt.v1", "receipt")
            .expect("minimum supported");
        validate_schema_version("shipper.receipt.v9", "shipper.receipt.v1", "receipt")
            .expect("newer versions");
    }

    #[test]
    fn validate_schema_version_rejects_older_versions() {
        let err = validate_schema_version("shipper.receipt.v0", "shipper.receipt.v1", "receipt")
            .expect_err("must fail");
        assert!(err.to_string().contains("too old"));
    }

    proptest! {
        #[test]
        fn parse_schema_version_roundtrips_number(version in 1u32..10_000) {
            let raw = format!("shipper.receipt.v{version}");
            prop_assert_eq!(parse_schema_version(&raw).expect("parse"), version);
        }

        #[test]
        fn validate_schema_version_accepts_equal_or_newer_versions(min in 1u32..5_000, offset in 0u32..5_000) {
            let actual = min.saturating_add(offset);
            let version = format!("shipper.receipt.v{actual}");
            let minimum = format!("shipper.receipt.v{min}");

            prop_assert!(validate_schema_version(&version, &minimum, "receipt").is_ok());
        }
    }
}
