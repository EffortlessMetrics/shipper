//! Cargo sparse-index helpers.
//!
//! This crate owns two focused concerns:
//! - Converting crate names to sparse-index paths
//! - Checking JSONL sparse-index content for a target version

use serde::Deserialize;

/// Compute the Cargo sparse-index path for a crate name.
///
/// Layout:
/// - `1/{name}` for length 1
/// - `2/{name}` for length 2
/// - `3/{name[0]}/{name}` for length 3
/// - `{name[0..2]}/{name[2..4]}/{name}` for length >= 4
///
/// Names are lowercased using ASCII rules.
pub fn sparse_index_path(crate_name: &str) -> String {
    let lower = crate_name.to_ascii_lowercase();
    match lower.len() {
        0 => "0/".to_string(),
        1 => format!("1/{}", lower),
        2 => format!("2/{}", lower),
        3 => format!("3/{}/{}", &lower[..1], lower),
        _ => format!("{}/{}/{}", &lower[..2], &lower[2..4], lower),
    }
}

#[derive(Debug, Deserialize)]
struct SparseIndexEntry {
    vers: String,
}

/// Returns `true` if JSONL sparse-index content contains the exact version.
///
/// Invalid lines are ignored.
pub fn contains_version(content: &str, version: &str) -> bool {
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<SparseIndexEntry>(line).ok())
        .any(|entry| entry.vers == version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_index_path_matches_cargo_layout() {
        assert_eq!(sparse_index_path("a"), "1/a");
        assert_eq!(sparse_index_path("ab"), "2/ab");
        assert_eq!(sparse_index_path("abc"), "3/a/abc");
        assert_eq!(sparse_index_path("demo"), "de/mo/demo");
    }

    #[test]
    fn sparse_index_path_lowercases_ascii_names() {
        assert_eq!(sparse_index_path("Serde"), "se/rd/serde");
        assert_eq!(sparse_index_path("A"), "1/a");
    }

    #[test]
    fn sparse_index_path_handles_empty_name_without_panicking() {
        assert_eq!(sparse_index_path(""), "0/");
    }

    #[test]
    fn contains_version_finds_exact_match() {
        let content = r#"{"vers":"0.1.0"}
{"vers":"1.0.0"}
{"vers":"2.0.0"}"#;
        assert!(contains_version(content, "1.0.0"));
        assert!(!contains_version(content, "3.0.0"));
    }

    #[test]
    fn contains_version_ignores_invalid_lines() {
        let content = r#"{"vers":"0.1.0"}
not json
{"oops":"missing-vers"}
{"vers":"1.2.3"}"#;
        assert!(contains_version(content, "1.2.3"));
    }

    #[test]
    fn contains_version_requires_exact_match() {
        let content = r#"{"vers":"1.2.3"}"#;
        assert!(!contains_version(content, "1.2"));
    }
}

#[cfg(test)]
mod property_tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn sparse_index_path_is_deterministic(name in "[A-Za-z0-9_-]{0,32}") {
            let first = sparse_index_path(&name);
            let second = sparse_index_path(&name);
            prop_assert_eq!(first, second);
        }

        #[test]
        fn sparse_index_path_ends_with_lowercase_name_for_non_empty_inputs(name in "[A-Za-z0-9_-]{1,32}") {
            let lower = name.to_ascii_lowercase();
            let path = sparse_index_path(&name);
            prop_assert!(path.ends_with(&lower));
        }

        #[test]
        fn contains_version_returns_true_when_version_is_present(
            target in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
            others in prop::collection::vec("[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}", 0..16),
        ) {
            let mut versions = Vec::with_capacity(others.len() + 1);
            versions.push(target.clone());
            versions.extend(others);

            let content = versions
                .iter()
                .map(|v| format!("{{\"vers\":\"{}\"}}", v))
                .collect::<Vec<_>>()
                .join("\n");

            prop_assert!(contains_version(&content, &target));
        }

        #[test]
        fn contains_version_returns_false_when_version_is_absent(
            target in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
            versions in prop::collection::vec("[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}", 0..16),
        ) {
            let unique: BTreeSet<String> = versions.into_iter().filter(|v| v != &target).collect();
            let content = unique
                .iter()
                .map(|v| format!("{{\"vers\":\"{}\"}}", v))
                .collect::<Vec<_>>()
                .join("\n");

            prop_assert_eq!(contains_version(&content, &target), unique.contains(&target));
        }
    }
}
