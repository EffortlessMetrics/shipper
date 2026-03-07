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
    use insta::assert_debug_snapshot;
    use proptest::prelude::*;

    #[test]
    fn parse_schema_version_extracts_numeric_suffix() {
        let parsed = parse_schema_version("shipper.receipt.v42").expect("parse");
        assert_eq!(parsed, 42);
    }

    // --- Additional parse edge-case tests ---

    #[test]
    fn parse_schema_version_accepts_v0() {
        assert_eq!(parse_schema_version("shipper.receipt.v0").unwrap(), 0);
    }

    #[test]
    fn parse_schema_version_accepts_leading_zeros() {
        // Rust's u32 parse treats "007" as 7
        assert_eq!(parse_schema_version("shipper.receipt.v007").unwrap(), 7);
    }

    #[test]
    fn parse_schema_version_rejects_empty_string() {
        assert!(parse_schema_version("").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_empty_version_after_v() {
        assert!(parse_schema_version("shipper.receipt.v").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_negative_version() {
        assert!(parse_schema_version("shipper.receipt.v-1").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_float_version() {
        assert!(parse_schema_version("shipper.receipt.v1.5").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_whitespace_around_input() {
        assert!(parse_schema_version(" shipper.receipt.v1 ").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_single_segment() {
        assert!(parse_schema_version("shipper").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_only_dots() {
        assert!(parse_schema_version("..").is_err());
    }

    #[test]
    fn parse_schema_version_accepts_u32_max() {
        let input = format!("shipper.receipt.v{}", u32::MAX);
        assert_eq!(parse_schema_version(&input).unwrap(), u32::MAX);
    }

    #[test]
    fn parse_schema_version_rejects_overflow_u32() {
        let overflow = u64::from(u32::MAX) + 1;
        let input = format!("shipper.receipt.v{overflow}");
        assert!(parse_schema_version(&input).is_err());
    }

    #[test]
    fn parse_schema_version_ignores_middle_segment_content() {
        // The middle segment can be anything; only prefix and version suffix matter
        assert_eq!(parse_schema_version("shipper.anything.v5").unwrap(), 5);
        assert_eq!(parse_schema_version("shipper..v5").unwrap(), 5);
    }

    // --- Additional validate edge-case tests ---

    #[test]
    fn validate_schema_version_accepts_both_zero() {
        validate_schema_version("shipper.receipt.v0", "shipper.receipt.v0", "receipt")
            .expect("v0 >= v0 should succeed");
    }

    #[test]
    fn validate_schema_version_does_not_compare_middle_segments() {
        // Middle segments differ (receipt vs state) — function only compares version numbers
        validate_schema_version("shipper.receipt.v3", "shipper.state.v2", "mixed")
            .expect("cross-segment comparison should still work");
    }

    #[test]
    fn validate_schema_version_fails_when_version_is_invalid() {
        let err = validate_schema_version("garbage", "shipper.receipt.v1", "receipt")
            .expect_err("must fail");
        assert!(err.to_string().contains("invalid receipt version format"));
    }

    #[test]
    fn validate_schema_version_fails_when_minimum_is_invalid() {
        let err = validate_schema_version("shipper.receipt.v1", "garbage", "receipt")
            .expect_err("must fail");
        assert!(err.to_string().contains("invalid minimum version format"));
    }

    #[test]
    fn validate_schema_version_label_appears_in_error_message() {
        let err = validate_schema_version("shipper.x.v0", "shipper.x.v5", "my_custom_label")
            .expect_err("must fail");
        let msg = err.to_string();
        assert!(msg.contains("my_custom_label"), "label missing from: {msg}");
    }

    // --- Snapshot tests using assert_debug_snapshot! ---

    #[test]
    fn snapshot_parse_ok_result() {
        assert_debug_snapshot!(parse_schema_version("shipper.receipt.v42"));
    }

    #[test]
    fn snapshot_parse_err_invalid_format() {
        assert_debug_snapshot!(parse_schema_version("invalid").map_err(|e| e.to_string()));
    }

    #[test]
    fn snapshot_parse_err_non_numeric() {
        assert_debug_snapshot!(
            parse_schema_version("shipper.receipt.vx").map_err(|e| e.to_string())
        );
    }

    #[test]
    fn snapshot_validate_ok() {
        assert_debug_snapshot!(validate_schema_version(
            "shipper.state.v3",
            "shipper.state.v1",
            "state"
        ));
    }

    #[test]
    fn snapshot_validate_err_too_old() {
        assert_debug_snapshot!(
            validate_schema_version("shipper.state.v0", "shipper.state.v5", "state")
                .map_err(|e| e.to_string())
        );
    }

    #[test]
    fn snapshot_parse_boundary_values() {
        let results: Vec<_> = [
            "shipper.x.v0",
            "shipper.x.v1",
            &format!("shipper.x.v{}", u32::MAX),
        ]
        .iter()
        .map(|s| (s.to_string(), parse_schema_version(s).ok()))
        .collect();
        assert_debug_snapshot!(results);
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

    // --- Version compatibility: sequential upgrade chain ---

    #[test]
    fn validate_upgrade_chain_v1_through_v5() {
        for version in 1u32..=5 {
            let v = format!("shipper.state.v{version}");
            let min = "shipper.state.v1";
            validate_schema_version(&v, min, "state")
                .unwrap_or_else(|_| panic!("v{version} should satisfy minimum v1"));
        }
    }

    #[test]
    fn validate_downgrade_always_rejected() {
        for (newer, older) in [(5, 4), (4, 3), (3, 2), (2, 1)] {
            let v = format!("shipper.state.v{older}");
            let min = format!("shipper.state.v{newer}");
            assert!(
                validate_schema_version(&v, &min, "state").is_err(),
                "v{older} should not satisfy minimum v{newer}"
            );
        }
    }

    #[test]
    fn validate_error_message_includes_both_versions() {
        let err = validate_schema_version("shipper.receipt.v1", "shipper.receipt.v5", "receipt")
            .expect_err("must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("v1"),
            "error should mention actual version: {msg}"
        );
        assert!(
            msg.contains("v5"),
            "error should mention minimum version: {msg}"
        );
    }

    #[test]
    fn validate_at_u32_max_boundary() {
        let max_ver = format!("shipper.receipt.v{}", u32::MAX);
        let min_ver = format!("shipper.receipt.v{}", u32::MAX);
        validate_schema_version(&max_ver, &min_ver, "receipt")
            .expect("u32::MAX should satisfy itself");
    }

    #[test]
    fn validate_both_arguments_invalid_returns_error() {
        let result = validate_schema_version("garbage", "also_garbage", "test");
        assert!(result.is_err());
    }

    // --- Edge cases: unusual/adversarial inputs ---

    #[test]
    fn parse_schema_version_accepts_shipper_prefix_superstring() {
        // "shippers" starts with "shipper" so the current impl accepts it
        assert_eq!(parse_schema_version("shippers.receipt.v3").unwrap(), 3);
    }

    #[test]
    fn parse_schema_version_rejects_uppercase_v_prefix() {
        assert!(parse_schema_version("shipper.receipt.V2").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_tab_separated() {
        assert!(parse_schema_version("shipper\treceipt\tv1").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_unicode_digit() {
        // U+0661 is Arabic-Indic digit one — not valid for u32::parse
        assert!(parse_schema_version("shipper.receipt.v\u{0661}").is_err());
    }

    #[test]
    fn parse_schema_version_rejects_version_with_trailing_text() {
        assert!(parse_schema_version("shipper.receipt.v2beta").is_err());
    }

    #[test]
    fn parse_schema_version_accepts_version_with_plus_sign() {
        // Rust's u32::parse treats "+1" as 1; document this accepted behavior
        assert_eq!(parse_schema_version("shipper.receipt.v+1").unwrap(), 1);
    }

    #[test]
    fn parse_schema_version_handles_very_long_middle_segment() {
        let long_middle = "a".repeat(10_000);
        let input = format!("shipper.{long_middle}.v7");
        assert_eq!(parse_schema_version(&input).unwrap(), 7);
    }

    #[test]
    fn parse_schema_version_deterministic_across_calls() {
        let input = "shipper.receipt.v42";
        let a = parse_schema_version(input).unwrap();
        let b = parse_schema_version(input).unwrap();
        assert_eq!(a, b);
    }

    // --- Snapshot tests ---

    #[test]
    fn snapshot_parse_multiple_document_types() {
        let types = ["receipt", "state", "events", "lock"];
        let results: Vec<_> = types
            .iter()
            .map(|t| {
                let input = format!("shipper.{t}.v1");
                (t.to_string(), parse_schema_version(&input).ok())
            })
            .collect();
        assert_debug_snapshot!(results);
    }

    #[test]
    fn snapshot_validate_upgrade_compatibility_matrix() {
        let versions: Vec<u32> = vec![0, 1, 2, 3, 5];
        let mut matrix: Vec<String> = Vec::new();
        for &v in &versions {
            for &min in &versions {
                let ver = format!("shipper.state.v{v}");
                let minimum = format!("shipper.state.v{min}");
                let ok = validate_schema_version(&ver, &minimum, "state").is_ok();
                matrix.push(format!("v{v} >= v{min}: {ok}"));
            }
        }
        assert_debug_snapshot!(matrix);
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

        #[test]
        fn parse_schema_version_never_panics_on_arbitrary_input(s in "\\PC*") {
            // Must not panic regardless of input; Ok or Err are both fine.
            let _ = parse_schema_version(&s);
        }

        #[test]
        fn validate_schema_version_never_panics_on_arbitrary_inputs(
            v in "\\PC*",
            m in "\\PC*",
            label in "[a-z]{1,10}",
        ) {
            let _ = validate_schema_version(&v, &m, &label);
        }

        #[test]
        fn parse_rejects_wrong_segment_count(
            a in "[a-z]{1,8}",
            b in "[a-z]{0,8}",
        ) {
            // Two segments: "a.b" should always be rejected.
            let two = format!("{a}.{b}");
            prop_assert!(parse_schema_version(&two).is_err());

            // Four segments: "a.b.c.d" should always be rejected.
            let four = format!("{a}.{b}.v1.extra");
            prop_assert!(parse_schema_version(&four).is_err());
        }

        #[test]
        fn parse_rejects_non_shipper_prefix(
            prefix in "[a-z]{1,8}".prop_filter("not shipper", |p| !p.starts_with("shipper")),
            middle in "[a-z]{1,8}",
            ver in 0u32..1_000,
        ) {
            let raw = format!("{prefix}.{middle}.v{ver}");
            prop_assert!(parse_schema_version(&raw).is_err());
        }

        #[test]
        fn parse_roundtrips_with_arbitrary_middle_segment(
            middle in "[a-z]{1,12}",
            ver in 0u32..100_000,
        ) {
            let raw = format!("shipper.{middle}.v{ver}");
            prop_assert_eq!(parse_schema_version(&raw).expect("parse"), ver);
        }

        #[test]
        fn validate_rejects_older_versions(
            min in 1u32..5_000,
            gap in 1u32..5_000,
        ) {
            let older = min.saturating_sub(gap);
            // Only meaningful when older < min (skip when saturated to 0 and min is 0).
            prop_assume!(older < min);
            let version = format!("shipper.state.v{older}");
            let minimum = format!("shipper.state.v{min}");
            prop_assert!(validate_schema_version(&version, &minimum, "state").is_err());
        }

        #[test]
        fn version_comparison_is_consistent(
            a in 0u32..10_000,
            b in 0u32..10_000,
        ) {
            let va = format!("shipper.receipt.v{a}");
            let vb = format!("shipper.receipt.v{b}");
            let a_ge_b = validate_schema_version(&va, &vb, "t").is_ok();
            let b_ge_a = validate_schema_version(&vb, &va, "t").is_ok();
            if a == b {
                prop_assert!(a_ge_b && b_ge_a);
            } else if a > b {
                prop_assert!(a_ge_b && !b_ge_a);
            } else {
                prop_assert!(!a_ge_b && b_ge_a);
            }
        }

        #[test]
        fn validate_is_transitive(
            a in 0u32..3_000,
            b in 0u32..3_000,
            c in 0u32..3_000,
        ) {
            let va = format!("shipper.state.v{a}");
            let vb = format!("shipper.state.v{b}");
            let vc = format!("shipper.state.v{c}");
            let a_ge_b = validate_schema_version(&va, &vb, "t").is_ok();
            let b_ge_c = validate_schema_version(&vb, &vc, "t").is_ok();
            let a_ge_c = validate_schema_version(&va, &vc, "t").is_ok();
            // Transitivity: if a >= b and b >= c then a >= c
            if a_ge_b && b_ge_c {
                prop_assert!(a_ge_c, "transitivity violated: v{a} >= v{b} and v{b} >= v{c} but not v{a} >= v{c}");
            }
        }

        #[test]
        fn parse_version_ordering_matches_numeric_ordering(
            a in 0u32..10_000,
            b in 0u32..10_000,
        ) {
            let pa = parse_schema_version(&format!("shipper.receipt.v{a}")).unwrap();
            let pb = parse_schema_version(&format!("shipper.receipt.v{b}")).unwrap();
            prop_assert_eq!(a.cmp(&b), pa.cmp(&pb));
        }

        /// Total ordering: for any two versions, exactly one of a>=b or b>a holds.
        #[test]
        fn version_total_ordering(a in 0u32..10_000, b in 0u32..10_000) {
            let va = format!("shipper.state.v{a}");
            let vb = format!("shipper.state.v{b}");
            let a_ge_b = validate_schema_version(&va, &vb, "t").is_ok();
            let b_ge_a = validate_schema_version(&vb, &va, "t").is_ok();
            // At least one must hold (totality), and both hold iff equal (antisymmetry)
            prop_assert!(a_ge_b || b_ge_a, "no ordering between v{a} and v{b}");
            if a == b {
                prop_assert!(a_ge_b && b_ge_a);
            }
        }

        /// Upgrade path: any version can be "upgraded" to u32::MAX (latest possible).
        #[test]
        fn any_version_upgradable_to_max(v in 0u32..=u32::MAX) {
            let version = format!("shipper.receipt.v{}", u32::MAX);
            let minimum = format!("shipper.receipt.v{v}");
            prop_assert!(validate_schema_version(&version, &minimum, "receipt").is_ok(),
                "u32::MAX should satisfy any minimum v{v}");
        }

        /// Self-validation: parsing a version and validating it against itself always succeeds.
        #[test]
        fn parse_then_validate_self_always_succeeds(v in 0u32..100_000) {
            let vs = format!("shipper.state.v{v}");
            let parsed = parse_schema_version(&vs).expect("parse");
            prop_assert_eq!(parsed, v);
            prop_assert!(validate_schema_version(&vs, &vs, "self").is_ok());
        }
    }
}
