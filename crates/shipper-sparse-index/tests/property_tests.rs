use std::collections::BTreeSet;

use proptest::prelude::*;
use shipper_sparse_index::{contains_version, sparse_index_path};

/// Strategy for valid crate names (ASCII alphanumeric, hyphens, underscores).
fn crate_name_strategy() -> impl Strategy<Value = String> {
    "[A-Za-z][A-Za-z0-9_-]{0,31}"
}

/// Strategy for semver-like version strings.
fn version_strategy() -> impl Strategy<Value = String> {
    "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}"
}

// ---------------------------------------------------------------------------
// sparse_index_path: index URL / path computation
// ---------------------------------------------------------------------------

proptest! {
    /// Case-insensitive: upper and lower name produce the same path.
    #[test]
    fn path_is_case_insensitive(name in crate_name_strategy()) {
        let lower_path = sparse_index_path(&name.to_ascii_lowercase());
        let upper_path = sparse_index_path(&name.to_ascii_uppercase());
        prop_assert_eq!(lower_path, upper_path);
    }

    /// The output path is always fully ASCII-lowercase.
    #[test]
    fn path_output_is_lowercase(name in crate_name_strategy()) {
        let path = sparse_index_path(&name);
        prop_assert_eq!(path.clone(), path.to_ascii_lowercase());
    }

    /// The path always uses forward-slash separators (never backslash).
    #[test]
    fn path_uses_forward_slashes(name in "[A-Za-z0-9_-]{0,32}") {
        let path = sparse_index_path(&name);
        prop_assert!(!path.contains('\\'));
    }

    /// Length-1 names produce "1/{name}".
    #[test]
    fn path_prefix_for_length_1(name in "[A-Za-z]") {
        let path = sparse_index_path(&name);
        prop_assert!(path.starts_with("1/"), "expected '1/' prefix, got {}", path);
    }

    /// Length-2 names produce "2/{name}".
    #[test]
    fn path_prefix_for_length_2(name in "[A-Za-z][A-Za-z0-9]") {
        let path = sparse_index_path(&name);
        prop_assert!(path.starts_with("2/"), "expected '2/' prefix, got {}", path);
    }

    /// Length-3 names produce "3/{first_char}/{name}".
    #[test]
    fn path_prefix_for_length_3(name in "[A-Za-z][A-Za-z0-9]{2}") {
        let path = sparse_index_path(&name);
        let lower = name.to_ascii_lowercase();
        let expected_prefix = format!("3/{}/", &lower[..1]);
        prop_assert!(
            path.starts_with(&expected_prefix),
            "expected prefix '{}', got '{}'", expected_prefix, path
        );
    }

    /// Length >= 4 names have the correct two-character prefix buckets.
    #[test]
    fn path_prefix_for_length_ge4(name in "[A-Za-z][A-Za-z0-9]{3,31}") {
        let path = sparse_index_path(&name);
        let lower = name.to_ascii_lowercase();
        let expected_prefix = format!("{}/{}/", &lower[..2], &lower[2..4]);
        prop_assert!(
            path.starts_with(&expected_prefix),
            "expected prefix '{}', got '{}'", expected_prefix, path
        );
    }

    /// The number of path segments matches the Cargo sparse-index spec.
    #[test]
    fn path_has_correct_segment_count(name in "[A-Za-z][A-Za-z0-9_-]{0,31}") {
        let path = sparse_index_path(&name);
        let segments: Vec<&str> = path.split('/').collect();
        let expected = match name.len() {
            1 => 2, // "1" / name
            2 => 2, // "2" / name
            3 => 3, // "3" / first_char / name
            _ => 3, // ab / cd / name
        };
        prop_assert_eq!(
            segments.len(), expected,
            "name={}, path={}, segments={:?}", name, path, segments
        );
    }

    /// No path segment is empty (except for the empty-name edge case).
    #[test]
    fn path_has_no_empty_segments(name in "[A-Za-z][A-Za-z0-9_-]{0,31}") {
        let path = sparse_index_path(&name);
        for segment in path.split('/') {
            prop_assert!(!segment.is_empty(), "empty segment in path '{}'", path);
        }
    }
}

// ---------------------------------------------------------------------------
// contains_version: entry parsing with random valid/invalid inputs
// ---------------------------------------------------------------------------

proptest! {
    /// Never panics on completely arbitrary content and version strings.
    #[test]
    fn contains_version_never_panics(content in ".*", version in ".*") {
        // Just exercise the function; any bool result is fine.
        let _ = contains_version(&content, &version);
    }

    /// Empty content never contains any version.
    #[test]
    fn empty_content_returns_false(version in version_strategy()) {
        prop_assert!(!contains_version("", &version));
    }

    /// Whitespace-only content never contains any version.
    #[test]
    fn whitespace_content_returns_false(
        ws in prop::collection::vec("[ \\t\\n]{1,8}", 1..8),
        version in version_strategy(),
    ) {
        let content = ws.join("");
        prop_assert!(!contains_version(&content, &version));
    }

    /// Injecting garbage lines between valid entries does not hide a version.
    #[test]
    fn garbage_lines_do_not_hide_version(
        target in version_strategy(),
        garbage in prop::collection::vec("[a-zA-Z0-9 {}:\"]{1,40}", 0..8),
    ) {
        let mut lines: Vec<String> = garbage;
        lines.push(format!("{{\"vers\":\"{}\"}}", target));
        // Rotate so the valid line isn't always last.
        let rotate = lines.len() / 2;
        lines.rotate_left(rotate);
        let content = lines.join("\n");
        prop_assert!(contains_version(&content, &target));
    }

    /// Looking up a version that was never inserted returns false.
    #[test]
    fn absent_version_returns_false(
        target in version_strategy(),
        others in prop::collection::vec(version_strategy(), 0..16),
    ) {
        let unique: BTreeSet<String> = others.into_iter().filter(|v| v != &target).collect();
        let content = unique
            .iter()
            .map(|v| format!("{{\"vers\":\"{}\"}}", v))
            .collect::<Vec<_>>()
            .join("\n");
        prop_assert!(!contains_version(&content, &target));
    }

    /// Result is independent of line ordering.
    #[test]
    fn result_is_order_independent(
        target in version_strategy(),
        mut versions in prop::collection::vec(version_strategy(), 1..16),
    ) {
        versions.push(target.clone());

        let forward_content = versions
            .iter()
            .map(|v| format!("{{\"vers\":\"{}\"}}", v))
            .collect::<Vec<_>>()
            .join("\n");

        let mut reversed = versions.clone();
        reversed.reverse();
        let reverse_content = reversed
            .iter()
            .map(|v| format!("{{\"vers\":\"{}\"}}", v))
            .collect::<Vec<_>>()
            .join("\n");

        prop_assert_eq!(
            contains_version(&forward_content, &target),
            contains_version(&reverse_content, &target),
        );
    }

    /// Extra JSON fields do not prevent version detection.
    #[test]
    fn extra_fields_do_not_break_parsing(
        target in version_strategy(),
        extra_key in "[a-z]{1,8}",
        extra_val in "[a-z0-9]{1,8}",
    ) {
        let content = format!(
            "{{\"{}\":\"{}\",\"vers\":\"{}\"}}",
            extra_key, extra_val, target
        );
        prop_assert!(contains_version(&content, &target));
    }

    /// A version string that is a substring of an entry does not false-match.
    #[test]
    fn substring_version_does_not_match(
        major in 0u16..100,
        minor in 0u16..100,
        patch in 0u16..100,
    ) {
        let full = format!("{}.{}.{}", major, minor, patch);
        let partial = format!("{}.{}", major, minor);
        let content = format!("{{\"vers\":\"{}\"}}", full);
        // Partial should only match if it happens to equal full.
        prop_assert_eq!(contains_version(&content, &partial), partial == full);
    }
}
