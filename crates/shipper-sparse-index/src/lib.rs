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

    // ── Index URL construction: boundary lengths ──

    #[test]
    fn sparse_index_path_exact_four_char_boundary() {
        assert_eq!(sparse_index_path("abcd"), "ab/cd/abcd");
    }

    #[test]
    fn sparse_index_path_five_chars() {
        assert_eq!(sparse_index_path("hello"), "he/ll/hello");
    }

    #[test]
    fn sparse_index_path_long_name() {
        let name = "a".to_string() + &"b".repeat(99);
        let path = sparse_index_path(&name);
        assert!(path.starts_with("ab/bb/"));
        assert!(path.ends_with(&name));
    }

    // ── Crate name edge cases ──

    #[test]
    fn sparse_index_path_with_hyphens() {
        assert_eq!(sparse_index_path("my-crate"), "my/-c/my-crate");
    }

    #[test]
    fn sparse_index_path_with_underscores() {
        assert_eq!(sparse_index_path("my_crate"), "my/_c/my_crate");
    }

    #[test]
    fn sparse_index_path_hyphen_underscore_produce_different_paths() {
        let hyphen = sparse_index_path("my-crate");
        let underscore = sparse_index_path("my_crate");
        assert_ne!(hyphen, underscore);
    }

    #[test]
    fn sparse_index_path_digits_in_name() {
        assert_eq!(sparse_index_path("h264"), "h2/64/h264");
        assert_eq!(sparse_index_path("3d"), "2/3d");
    }

    #[test]
    fn sparse_index_path_all_digits() {
        assert_eq!(sparse_index_path("1234"), "12/34/1234");
    }

    #[test]
    #[should_panic(expected = "byte index")]
    fn sparse_index_path_panics_on_multibyte_unicode() {
        // Crate names must be ASCII; multi-byte chars cause an indexing panic
        let _ = sparse_index_path("café");
    }

    #[test]
    fn sparse_index_path_ascii_only_unicode_safe() {
        // Pure ASCII with non-alpha chars does not panic
        let path = sparse_index_path("a-b_c");
        assert_eq!(path, "a-/b_/a-b_c");
    }

    #[test]
    fn sparse_index_path_mixed_case_three_char() {
        assert_eq!(sparse_index_path("SYN"), "3/s/syn");
        assert_eq!(sparse_index_path("Syn"), "3/s/syn");
    }

    #[test]
    fn sparse_index_path_already_lowercase() {
        assert_eq!(sparse_index_path("serde"), sparse_index_path("SERDE"));
    }

    #[test]
    fn sparse_index_path_single_char_variants() {
        for c in b'A'..=b'Z' {
            let upper = String::from(c as char);
            let lower = upper.to_ascii_lowercase();
            assert_eq!(sparse_index_path(&upper), format!("1/{lower}"));
        }
    }

    // ── Response parsing edge cases ──

    #[test]
    fn contains_version_empty_content() {
        assert!(!contains_version("", "1.0.0"));
    }

    #[test]
    fn contains_version_whitespace_only_content() {
        assert!(!contains_version("   \t  \n  \n  ", "1.0.0"));
    }

    #[test]
    fn contains_version_single_entry() {
        assert!(contains_version(r#"{"vers":"0.1.0"}"#, "0.1.0"));
    }

    #[test]
    fn contains_version_many_versions() {
        let content: String = (0..200)
            .map(|i| format!("{{\"vers\":\"0.{i}.0\"}}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(contains_version(&content, "0.99.0"));
        assert!(contains_version(&content, "0.0.0"));
        assert!(contains_version(&content, "0.199.0"));
        assert!(!contains_version(&content, "0.200.0"));
    }

    #[test]
    fn contains_version_prerelease() {
        let content = r#"{"vers":"1.0.0-alpha.1"}
{"vers":"1.0.0-beta.2"}
{"vers":"1.0.0"}"#;
        assert!(contains_version(content, "1.0.0-alpha.1"));
        assert!(contains_version(content, "1.0.0-beta.2"));
        assert!(contains_version(content, "1.0.0"));
        assert!(!contains_version(content, "1.0.0-rc.1"));
    }

    #[test]
    fn contains_version_build_metadata() {
        let content = r#"{"vers":"1.0.0+build.123"}"#;
        assert!(contains_version(content, "1.0.0+build.123"));
        assert!(!contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_trailing_newline() {
        let content = "{\"vers\":\"1.0.0\"}\n";
        assert!(contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_blank_lines_between_entries() {
        let content = "{\"vers\":\"0.1.0\"}\n\n\n{\"vers\":\"0.2.0\"}\n\n";
        assert!(contains_version(content, "0.1.0"));
        assert!(contains_version(content, "0.2.0"));
    }

    #[test]
    fn contains_version_windows_line_endings() {
        let content = "{\"vers\":\"0.1.0\"}\r\n{\"vers\":\"0.2.0\"}\r\n";
        assert!(contains_version(content, "0.1.0"));
        assert!(contains_version(content, "0.2.0"));
    }

    #[test]
    fn contains_version_empty_version_query() {
        let content = r#"{"vers":"1.0.0"}"#;
        assert!(!contains_version(content, ""));
    }

    #[test]
    fn contains_version_duplicate_versions() {
        let content = "{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.0.0\"}";
        assert!(contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_realistic_full_entry() {
        let content = r#"{"name":"serde","vers":"1.0.210","deps":[{"name":"serde_derive","req":"^1.0","features":["default"],"optional":true,"default_features":false,"target":null,"kind":"normal"}],"cksum":"abcdef1234567890","features":{"default":["std"],"derive":["serde_derive"],"std":[]},"yanked":false,"links":null,"v":2}"#;
        assert!(contains_version(content, "1.0.210"));
        assert!(!contains_version(content, "1.0.211"));
    }

    #[test]
    fn contains_version_yanked_entry_still_matches() {
        let content = r#"{"vers":"0.1.0","yanked":true}"#;
        assert!(contains_version(content, "0.1.0"));
    }

    // ── Simulated error responses (non-JSON content) ──

    #[test]
    fn contains_version_html_error_page() {
        let content = "<html><body>404 Not Found</body></html>";
        assert!(!contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_plain_text_error() {
        let content = "rate limit exceeded";
        assert!(!contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_json_error_object() {
        let content = r#"{"errors":[{"detail":"Not Found"}]}"#;
        assert!(!contains_version(content, "1.0.0"));
    }

    // ── Version filtering precision ──

    #[test]
    fn contains_version_does_not_match_prefix() {
        let content = r#"{"vers":"1.0.0"}"#;
        assert!(!contains_version(content, "1.0"));
        assert!(!contains_version(content, "1"));
    }

    #[test]
    fn contains_version_does_not_match_suffix() {
        let content = r#"{"vers":"1.0.0"}"#;
        assert!(!contains_version(content, "0.0"));
        assert!(!contains_version(content, "1.0.0.0"));
    }

    #[test]
    fn contains_version_distinguishes_similar_versions() {
        let content = r#"{"vers":"1.10.0"}
{"vers":"1.1.0"}
{"vers":"10.1.0"}"#;
        assert!(contains_version(content, "1.10.0"));
        assert!(contains_version(content, "1.1.0"));
        assert!(contains_version(content, "10.1.0"));
        assert!(!contains_version(content, "1.0.0"));
        assert!(!contains_version(content, "1.100.0"));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use insta::assert_snapshot;

    // ── sparse_index_path: all length categories ──

    #[test]
    fn snapshot_path_empty_name() {
        assert_snapshot!(sparse_index_path(""), @"0/");
    }

    #[test]
    fn snapshot_path_one_char() {
        assert_snapshot!(sparse_index_path("a"), @"1/a");
    }

    #[test]
    fn snapshot_path_two_chars() {
        assert_snapshot!(sparse_index_path("ab"), @"2/ab");
    }

    #[test]
    fn snapshot_path_three_chars() {
        assert_snapshot!(sparse_index_path("abc"), @"3/a/abc");
    }

    #[test]
    fn snapshot_path_four_chars() {
        assert_snapshot!(sparse_index_path("demo"), @"de/mo/demo");
    }

    // ── sparse_index_path: real-world crates ──

    #[test]
    fn snapshot_path_real_world_crates() {
        let crates = [
            "serde",
            "tokio",
            "clap",
            "anyhow",
            "rand",
            "syn",
            "proc-macro2",
            "quote",
            "libc",
            "regex",
        ];
        let paths: Vec<String> = crates
            .iter()
            .map(|c| format!("{c} -> {}", sparse_index_path(c)))
            .collect();
        assert_snapshot!(paths.join("\n"));
    }

    // ── sparse_index_path: case normalisation ──

    #[test]
    fn snapshot_path_mixed_case() {
        assert_snapshot!(sparse_index_path("Serde"), @"se/rd/serde");
    }

    #[test]
    fn snapshot_path_all_upper() {
        assert_snapshot!(sparse_index_path("TOKIO"), @"to/ki/tokio");
    }

    // ── sparse_index_path: index URL construction ──

    #[test]
    fn snapshot_full_sparse_index_url() {
        let base = "https://index.crates.io/";
        let crates = ["serde", "a", "ab", "syn", "rand_core"];
        let urls: Vec<String> = crates
            .iter()
            .map(|c| format!("{base}{}", sparse_index_path(c)))
            .collect();
        assert_snapshot!(urls.join("\n"));
    }

    // ── contains_version: parsed entry snapshots ──

    #[test]
    fn snapshot_version_found() {
        let content = r#"{"vers":"0.1.0"}
{"vers":"1.0.0"}
{"vers":"2.0.0"}"#;
        assert_snapshot!(contains_version(content, "1.0.0").to_string(), @"true");
    }

    #[test]
    fn snapshot_version_not_found() {
        let content = r#"{"vers":"0.1.0"}
{"vers":"1.0.0"}"#;
        assert_snapshot!(contains_version(content, "3.0.0").to_string(), @"false");
    }

    #[test]
    fn snapshot_version_with_extra_fields() {
        let content = r#"{"name":"serde","vers":"1.0.210","deps":[],"cksum":"abc","features":{},"yanked":false}
{"name":"serde","vers":"1.0.211","deps":[],"cksum":"def","features":{},"yanked":false}"#;
        assert_snapshot!(contains_version(content, "1.0.210").to_string(), @"true");
    }

    #[test]
    fn snapshot_version_with_invalid_lines() {
        let content = r#"not-json
{"vers":"0.5.0"}
{"oops":"missing"}
{"vers":"1.2.3"}"#;
        let results: Vec<String> = ["0.5.0", "1.2.3", "9.9.9"]
            .iter()
            .map(|v| format!("{v} -> {}", contains_version(content, v)))
            .collect();
        assert_snapshot!(results.join("\n"));
    }

    #[test]
    fn snapshot_version_empty_content() {
        assert_snapshot!(contains_version("", "1.0.0").to_string(), @"false");
    }

    // ── Additional snapshot tests ──

    #[test]
    fn snapshot_path_hyphenated_and_underscored_crates() {
        let crates = [
            "my-crate",
            "my_crate",
            "proc-macro2",
            "rand_core",
            "serde_json",
            "async-trait",
        ];
        let paths: Vec<String> = crates
            .iter()
            .map(|c| format!("{c} -> {}", sparse_index_path(c)))
            .collect();
        assert_snapshot!(paths.join("\n"));
    }

    #[test]
    fn snapshot_path_boundary_lengths() {
        let names = ["x", "ab", "syn", "clap", "tokio", "serde_json"];
        let paths: Vec<String> = names
            .iter()
            .map(|c| format!("len={} {c} -> {}", c.len(), sparse_index_path(c)))
            .collect();
        assert_snapshot!(paths.join("\n"));
    }

    #[test]
    fn snapshot_multiversion_lookup_results() {
        let content = r#"{"vers":"0.1.0"}
{"vers":"0.2.0"}
{"vers":"1.0.0-alpha"}
{"vers":"1.0.0"}
{"vers":"1.0.1"}
{"vers":"2.0.0"}"#;
        let queries = [
            "0.1.0",
            "0.2.0",
            "0.3.0",
            "1.0.0-alpha",
            "1.0.0",
            "1.0.1",
            "1.0.2",
            "2.0.0",
            "3.0.0",
        ];
        let results: Vec<String> = queries
            .iter()
            .map(|v| format!("{v} -> {}", contains_version(content, v)))
            .collect();
        assert_snapshot!(results.join("\n"));
    }

    #[test]
    fn snapshot_index_url_all_length_categories() {
        let base = "https://index.crates.io/";
        let names = ["x", "ab", "syn", "rand", "serde", "my-crate", "proc-macro2"];
        let urls: Vec<String> = names
            .iter()
            .map(|c| format!("{c} -> {base}{}", sparse_index_path(c)))
            .collect();
        assert_snapshot!(urls.join("\n"));
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

        #[test]
        fn sparse_index_path_correct_prefix_by_length(name in "[a-z][a-z0-9]{0,31}") {
            let path = sparse_index_path(&name);
            match name.len() {
                1 => prop_assert!(path.starts_with("1/"), "expected '1/' for len=1, got {path}"),
                2 => prop_assert!(path.starts_with("2/"), "expected '2/' for len=2, got {path}"),
                3 => {
                    let expected = format!("3/{}/", &name[..1]);
                    prop_assert!(path.starts_with(&expected), "expected '{expected}', got {path}");
                }
                n if n >= 4 => {
                    let expected = format!("{}/{}/", &name[..2], &name[2..4]);
                    prop_assert!(path.starts_with(&expected), "expected '{expected}', got {path}");
                }
                _ => {}
            }
        }

        #[test]
        fn contains_version_roundtrip_single(ver in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}") {
            let content = format!("{{\"vers\":\"{ver}\"}}");
            prop_assert!(contains_version(&content, &ver));
        }
    }
}
