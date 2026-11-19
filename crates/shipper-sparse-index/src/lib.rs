//! Cargo sparse-index helpers.
//!
//! This crate owns two focused concerns:
//! - Converting crate names to sparse-index paths
//! - Checking JSONL sparse-index content for a target version

use serde::Deserialize;

/// Compute the Cargo sparse-index path for a crate name.
///
/// Layout, measured in Unicode scalar values after lowercasing:
/// - `1/{name}` for length 1
/// - `2/{name}` for length 2
/// - `3/{first}/{name}` for length 3
/// - `{first two}/{next two}/{name}` for length >= 4
///
/// This helper performs mechanical path derivation only. Individual registries
/// may impose stricter crate-name policies, including ASCII-only names.
pub fn sparse_index_path(crate_name: &str) -> String {
    let lower = crate_name.to_lowercase();
    match lower.chars().count() {
        0 => "0/".to_string(),
        1 => format!("1/{}", lower),
        2 => format!("2/{}", lower),
        3 => {
            let first = lower.chars().next().into_iter().collect::<String>();
            format!("3/{first}/{lower}")
        }
        _ => {
            let first_two = lower.chars().take(2).collect::<String>();
            let next_two = lower.chars().skip(2).take(2).collect::<String>();
            format!("{first_two}/{next_two}/{lower}")
        }
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

/// Observe version presence without treating unreadable records as absence.
///
/// A valid matching record proves presence even if another record is malformed.
/// Absence requires at least one record and every nonblank record to contain a
/// valid Cargo semantic version. An empty or malformed response cannot authorize
/// another upload. Callers with a conclusive HTTP 404 should handle that absence
/// at the transport boundary.
/// The permissive [`contains_version`] helper retains its existing contract.
pub fn observe_version(content: &str, version: &str) -> Result<bool, InvalidIndexObservation> {
    let mut records = 0;
    let mut invalid = false;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        records += 1;
        match serde_json::from_str::<SparseIndexEntry>(line) {
            Ok(entry) if semver::Version::parse(&entry.vers).is_ok() => {
                if entry.vers == version {
                    return Ok(true);
                }
            }
            _ => invalid = true,
        }
    }
    if records == 0 || invalid {
        Err(InvalidIndexObservation)
    } else {
        Ok(false)
    }
}

/// The index response could not establish absence. Contains no response bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidIndexObservation;

impl std::fmt::Display for InvalidIndexObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sparse index contains empty or invalid version records")
    }
}

impl std::error::Error for InvalidIndexObservation {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_version_distinguishes_valid_absence_from_unreadable_records() {
        assert_eq!(observe_version(r#"{"vers":"1.0.0"}"#, "2.0.0"), Ok(false));
        for content in [
            "",
            " \n\t",
            "not json",
            r#"{"error":"secret"}"#,
            r#"{"vers":null}"#,
            r#"[{"vers":"1.0.0"}]"#,
        ] {
            assert_eq!(
                observe_version(content, "2.0.0"),
                Err(InvalidIndexObservation)
            );
        }
        let mixed = "not json\n{\"vers\":\"1.0.0\"}\n";
        assert_eq!(observe_version(mixed, "1.0.0"), Ok(true));
        assert_eq!(
            observe_version(mixed, "2.0.0"),
            Err(InvalidIndexObservation)
        );
        assert!(!InvalidIndexObservation.to_string().contains("secret"));
    }

    #[test]
    fn observe_version_rejects_invalid_semver_without_losing_valid_presence() {
        for invalid in [
            "",
            "not-a-version",
            "1.0",
            "01.0.0",
            "1.0.0-",
            "1.0.0+",
            " 1.0.0",
            "1.0.0\n",
        ] {
            let malformed = serde_json::json!({ "vers": invalid }).to_string();
            assert_eq!(
                observe_version(&malformed, "2.0.0"),
                Err(InvalidIndexObservation),
                "invalid version {invalid:?} must not prove absence"
            );
            assert_eq!(
                observe_version(&malformed, invalid),
                Err(InvalidIndexObservation)
            );
            assert!(
                contains_version(&malformed, invalid),
                "legacy search remains permissive"
            );
            for mixed in [
                format!("{malformed}\n{{\"vers\":\"1.0.0\"}}"),
                format!("{{\"vers\":\"1.0.0\"}}\n{malformed}"),
            ] {
                assert_eq!(observe_version(&mixed, "1.0.0"), Ok(true));
                assert_eq!(
                    observe_version(&mixed, "2.0.0"),
                    Err(InvalidIndexObservation)
                );
            }
        }
        for valid in ["0.0.0", "1.2.3-alpha.1+build.7", "2.0.0+build.42"] {
            let content = serde_json::json!({ "vers": valid }).to_string();
            assert_eq!(observe_version(&content, valid), Ok(true));
            assert_eq!(observe_version(&content, "9.9.9"), Ok(false));
        }
        assert_eq!(
            observe_version(r#"{"vers":"1.2.3+build.7"}"#, "1.2.3+build.8"),
            Ok(false),
            "semantic validation must preserve exact version-string matching"
        );
    }

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
    fn sparse_index_path_handles_multibyte_unicode_by_character() {
        assert_eq!(sparse_index_path("é"), "1/é");
        assert_eq!(sparse_index_path("éß"), "2/éß");
        assert_eq!(sparse_index_path("åßç"), "3/å/åßç");
        assert_eq!(sparse_index_path("café"), "ca/fé/café");
        assert_eq!(sparse_index_path("éclair"), "éc/la/éclair");
    }

    #[test]
    fn sparse_index_path_shards_after_unicode_lowercasing() {
        assert_eq!(sparse_index_path("İX"), "3/i/i̇x");
    }

    #[test]
    fn sparse_index_path_treats_combining_marks_as_distinct_scalars() {
        assert_eq!(sparse_index_path("éab"), "3/é/éab");
        assert_eq!(sparse_index_path("e\u{301}ab"), "e\u{301}/ab/e\u{301}ab");
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

    // ── JSONL whitespace tolerance ──

    #[test]
    fn contains_version_tolerates_trailing_whitespace_on_line() {
        let content = "{\"vers\":\"1.0.0\"}   \n";
        assert!(contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_tolerates_leading_whitespace_on_line() {
        let content = "  {\"vers\":\"1.0.0\"}\n";
        assert!(contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_tolerates_leading_and_trailing_whitespace() {
        let content = "  {\"vers\":\"1.0.0\"}   \n";
        assert!(contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_tolerates_tabs_around_record() {
        let content = "\t{\"vers\":\"1.0.0\"}\t\n";
        assert!(contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_inner_space_in_vers_does_not_match_trimmed_query() {
        let content = r#"{"vers":"1.0.0 "}"#;
        assert!(!contains_version(content, "1.0.0"));
        assert!(contains_version(content, "1.0.0 "));
    }

    #[test]
    fn contains_version_null_byte_in_vers_does_not_panic() {
        let content = "{\"vers\":\"1.0.0\\u0000\"}";
        assert!(!contains_version(content, "1.0.0"));
        assert!(contains_version(content, "1.0.0\0"));
    }

    #[test]
    fn contains_version_mixed_lf_and_crlf_line_endings() {
        let content = "{\"vers\":\"0.1.0\"}\r\n{\"vers\":\"0.2.0\"}\n{\"vers\":\"0.3.0\"}\r\n";
        assert!(contains_version(content, "0.1.0"));
        assert!(contains_version(content, "0.2.0"));
        assert!(contains_version(content, "0.3.0"));
    }

    #[test]
    fn contains_version_extra_fields_per_record_are_tolerated() {
        let content = r#"{"name":"foo","vers":"1.0.0","yanked":true,"deps":[{"name":"bar","req":"^1"}],"cksum":"deadbeef","features":{"default":[]},"v":2}"#;
        assert!(contains_version(content, "1.0.0"));
    }

    #[test]
    fn contains_version_yanked_versions_still_match() {
        let content = r#"{"vers":"0.1.0","yanked":true}
{"vers":"0.2.0","yanked":false}"#;
        assert!(contains_version(content, "0.1.0"));
        assert!(contains_version(content, "0.2.0"));
    }

    // ── sparse_index_path: more crate-name edges ──

    #[test]
    fn sparse_index_path_empty_name_returns_zero_slash() {
        assert_eq!(sparse_index_path(""), "0/");
    }

    #[test]
    fn sparse_index_path_three_digit_only_name() {
        assert_eq!(sparse_index_path("123"), "3/1/123");
    }

    #[test]
    fn sparse_index_path_long_hyphen_underscore_mixed() {
        assert_eq!(
            sparse_index_path("foo-bar-baz_qux"),
            "fo/o-/foo-bar-baz_qux"
        );
    }

    #[test]
    fn sparse_index_path_symbol_only_two_char_name() {
        assert_eq!(sparse_index_path("--"), "2/--");
        assert_eq!(sparse_index_path("__"), "2/__");
    }

    #[test]
    fn sparse_index_path_symbol_only_four_char_name() {
        assert_eq!(sparse_index_path("-_-_"), "-_/-_/-_-_");
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
