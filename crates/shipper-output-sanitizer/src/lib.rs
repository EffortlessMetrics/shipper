//! Output sanitization helpers for cargo command logs and evidence payloads.

/// Return the last `n` lines from `s`, then apply sensitive redaction.
///
/// # Examples
///
/// ```
/// use shipper_output_sanitizer::tail_lines;
///
/// let log = "line1\nline2\nline3\nline4";
/// assert_eq!(tail_lines(log, 2), "line3\nline4");
/// ```
pub fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let tail = if lines.len() <= n {
        s.to_string()
    } else {
        lines[lines.len() - n..].join("\n")
    };
    redact_sensitive(&tail)
}

/// Redact sensitive token-like patterns from output lines.
///
/// Applied to stdout/stderr tails before they are persisted.
///
/// # Examples
///
/// ```
/// use shipper_output_sanitizer::redact_sensitive;
///
/// assert_eq!(
///     redact_sensitive("CARGO_REGISTRY_TOKEN=secret123"),
///     "CARGO_REGISTRY_TOKEN=[REDACTED]"
/// );
///
/// // Non-sensitive content passes through unchanged
/// assert_eq!(
///     redact_sensitive("Compiling demo v0.1.0"),
///     "Compiling demo v0.1.0"
/// );
/// ```
pub fn redact_sensitive(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for line in s.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&redact_line(line));
    }
    // Preserve trailing newline if present.
    if s.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn redact_line(line: &str) -> String {
    let mut out = line.to_string();

    if let Some(pos) = out.to_ascii_lowercase().find("authorization:") {
        let after = &out[pos..];
        if let Some(bearer_pos) = after.to_ascii_lowercase().find("bearer ") {
            let redact_start = pos + bearer_pos + "bearer ".len();
            out = format!("{}[REDACTED]", &out[..redact_start]);
        }
    }

    if let Some(pos) = out.to_ascii_lowercase().find("token") {
        let after_key = &out[pos + "token".len()..];
        let trimmed = after_key.trim_start();
        if trimmed.starts_with("= ") || trimmed.starts_with("=") {
            let eq_offset = pos + "token".len() + (after_key.len() - trimmed.len());
            let after_eq = trimmed.trim_start_matches('=').trim_start();
            if after_eq.starts_with('"') || after_eq.starts_with('\'') {
                out = format!("{}= \"[REDACTED]\"", &out[..eq_offset]);
            } else if !after_eq.is_empty() {
                out = format!("{}= [REDACTED]", &out[..eq_offset]);
            }
        }
    }

    if let Some(pos) = find_cargo_token_env(&out)
        && let Some(eq_pos) = out[pos..].find('=')
    {
        let abs_eq = pos + eq_pos;
        out = format!("{}=[REDACTED]", &out[..abs_eq]);
    }

    out
}

fn find_cargo_token_env(s: &str) -> Option<usize> {
    if let Some(pos) = s.find("CARGO_REGISTRY_TOKEN") {
        return Some(pos);
    }
    if let Some(pos) = s.find("CARGO_REGISTRIES_") {
        let after = &s[pos + "CARGO_REGISTRIES_".len()..];
        if after.contains("_TOKEN") {
            return Some(pos);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_authorization_bearer_header() {
        let input = "Authorization: Bearer cio_abc123secret";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_token_assignment_quoted() {
        let input = r#"token = "cio_mysecrettoken""#;
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("cio_mysecrettoken"));
    }

    #[test]
    fn redact_cargo_registry_token_env() {
        let input = "CARGO_REGISTRY_TOKEN=cio_secret123";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_cargo_registries_named_token_env() {
        let input = "CARGO_REGISTRIES_MY_REG_TOKEN=secret456";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRIES_MY_REG_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_preserves_non_sensitive_content() {
        let input = "Compiling demo v0.1.0\nFinished release target";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn tail_lines_takes_last_lines_then_redacts() {
        let input = "first\nAuthorization: Bearer secret_token\nthird";
        let out = tail_lines(input, 2);
        assert_eq!(out, "Authorization: Bearer [REDACTED]\nthird");
    }

    #[test]
    fn tail_lines_with_more_lines_than_input_returns_whole_tail() {
        let input = "one\ntwo\nthree";
        assert_eq!(tail_lines(input, 10), input);
    }

    // --- redact_sensitive edge cases ---

    #[test]
    fn redact_empty_input() {
        assert_eq!(redact_sensitive(""), "");
    }

    #[test]
    fn redact_very_long_token_value() {
        let long_token = "a".repeat(2000);
        let input = format!("CARGO_REGISTRY_TOKEN={long_token}");
        let out = redact_sensitive(&input);
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
        assert!(!out.contains(&long_token));
    }

    #[test]
    fn redact_unicode_surrounding_text() {
        let input = "日本語 Authorization: Bearer secret_tok 中文";
        let out = redact_sensitive(input);
        assert_eq!(out, "日本語 Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_token_at_string_start() {
        let input = "token = abc123";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("abc123"));
    }

    #[test]
    fn redact_multiple_sensitive_patterns_one_line() {
        // Both "token=" and "CARGO_REGISTRY_TOKEN=" appear; at least one is redacted.
        let input = "CARGO_REGISTRY_TOKEN=secret1 token = secret2";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("secret1"));
    }

    #[test]
    fn redact_token_single_quoted() {
        let input = "token = 'my_secret_value'";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("my_secret_value"));
    }

    #[test]
    fn redact_token_unquoted_value() {
        let input = "token = plainvalue";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("plainvalue"));
    }

    #[test]
    fn redact_token_no_spaces_around_equals() {
        let input = "token=nospacesecret";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("nospacesecret"));
    }

    #[test]
    fn redact_authorization_case_insensitive() {
        let input = "authorization: bearer my_secret";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("my_secret"));
    }

    #[test]
    fn redact_preserves_trailing_newline() {
        let input = "CARGO_REGISTRY_TOKEN=secret\n";
        let out = redact_sensitive(input);
        assert!(out.ends_with('\n'));
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]\n");
    }

    #[test]
    fn redact_token_with_empty_value_after_equals() {
        // "token =" with nothing after — no redaction needed since value is empty.
        let input = "token = ";
        let out = redact_sensitive(input);
        // The trimmed after-eq is empty, so no replacement occurs.
        assert_eq!(out, "token = ");
    }

    #[test]
    fn redact_cargo_registries_without_token_suffix_not_matched() {
        // CARGO_REGISTRIES_FOO=bar has no _TOKEN suffix, should not be redacted.
        let input = "CARGO_REGISTRIES_FOO=bar";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRIES_FOO=bar");
    }

    #[test]
    fn redact_mixed_case_authorization() {
        let input = "AUTHORIZATION: BEARER top_secret";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("top_secret"));
    }

    #[test]
    fn redact_multiline_preserves_all_lines() {
        let input = "line1\nline2\nline3";
        let out = redact_sensitive(input);
        assert_eq!(out.lines().count(), 3);
    }

    // --- tail_lines edge cases ---

    #[test]
    fn tail_lines_empty_input() {
        assert_eq!(tail_lines("", 5), "");
    }

    #[test]
    fn tail_lines_zero_lines_requested() {
        let out = tail_lines("one\ntwo\nthree", 0);
        assert_eq!(out, "");
    }

    #[test]
    fn tail_lines_newline_only_input() {
        let out = tail_lines("\n", 5);
        // "\n" has one empty line via .lines(), plus trailing newline
        assert_eq!(out, "\n");
    }

    #[test]
    fn tail_lines_single_line_input() {
        assert_eq!(tail_lines("hello", 1), "hello");
    }

    #[test]
    fn tail_lines_sensitive_data_before_cutoff_excluded() {
        let input = "CARGO_REGISTRY_TOKEN=secret\nsafe line\nanother safe";
        let out = tail_lines(input, 2);
        assert!(!out.contains("CARGO_REGISTRY_TOKEN"));
        assert!(!out.contains("secret"));
        assert_eq!(out, "safe line\nanother safe");
    }

    #[test]
    fn tail_lines_preserves_trailing_newline_when_all_lines() {
        let input = "one\ntwo\n";
        let out = tail_lines(input, 10);
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn tail_lines_exact_count_match() {
        let input = "a\nb\nc";
        assert_eq!(tail_lines(input, 3), "a\nb\nc");
    }
}

#[cfg(test)]
mod property_tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn redact_sensitive_is_idempotent(input in ".*") {
            let once = redact_sensitive(&input);
            let twice = redact_sensitive(&once);
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn tail_lines_preserves_last_n_lines(
            lines in prop::collection::vec("[A-Za-z0-9 ]{0,12}", 0..12),
            n in 0usize..8,
            tail_newline in prop::bool::ANY,
        ) {
            let joined = lines.join("\n");
            let input = if tail_newline && !joined.is_empty() {
                format!("{}\n", joined)
            } else {
                joined
            };

            let result = tail_lines(&input, n);
            let expected_tail = if input.lines().count() <= n {
                input.lines().collect::<Vec<_>>()
            } else {
                input.lines().collect::<Vec<_>>()[input.lines().count() - n..].to_vec()
            };

            let expected = expected_tail.iter().fold(String::new(), |mut acc, line| {
                if !acc.is_empty() {
                    acc.push('\n');
                }
                acc.push_str(&redact_line(line));
                acc
            });
            let expected = if input.ends_with('\n') && input.lines().count() <= n {
                format!("{expected}\n")
            } else {
                expected
            };

            prop_assert_eq!(result, expected);
        }

        #[test]
        fn authorization_tokens_are_redacted(prefix in "[A-Za-z ]{0,12}", token in "[A-Za-z0-9_./-]{1,24}") {
            let input = format!("{prefix}Authorization: Bearer {token}");
            let out = redact_sensitive(&input);
            prop_assert!(out.contains("[REDACTED]"));
            prop_assert!(out.ends_with("Bearer [REDACTED]"), "Expected output to end with 'Bearer [REDACTED]', got: {}", out);
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn snapshot_redact_bearer_token() {
        assert_snapshot!(redact_sensitive("Authorization: Bearer cio_abc123secret"));
    }

    #[test]
    fn snapshot_redact_cargo_registry_token() {
        assert_snapshot!(redact_sensitive("CARGO_REGISTRY_TOKEN=mysecrettoken"));
    }

    #[test]
    fn snapshot_redact_named_registry_token() {
        assert_snapshot!(redact_sensitive(
            "CARGO_REGISTRIES_PRIVATE_REG_TOKEN=secret456"
        ));
    }

    #[test]
    fn snapshot_redact_token_assignment() {
        assert_snapshot!(redact_sensitive(r#"token = "cio_mysecrettoken""#));
    }

    #[test]
    fn snapshot_passthrough_normal_output() {
        assert_snapshot!(redact_sensitive(
            "Compiling demo v0.1.0\nFinished release target\nUploading to crates.io"
        ));
    }

    #[test]
    fn snapshot_tail_lines_3() {
        assert_snapshot!(tail_lines("line1\nline2\nline3\nline4\nline5", 3));
    }

    #[test]
    fn snapshot_tail_lines_with_redaction() {
        assert_snapshot!(tail_lines(
            "normal line\nCARGO_REGISTRY_TOKEN=secret\nfinal line",
            2
        ));
    }

    #[test]
    fn snapshot_mixed_sensitive_output() {
        let input =
            "Compiling foo\nAuthorization: Bearer secret123\nCARGO_REGISTRY_TOKEN=tok\nDone";
        assert_snapshot!(redact_sensitive(input));
    }

    #[test]
    fn snapshot_redact_empty() {
        assert_snapshot!(redact_sensitive(""));
    }

    #[test]
    fn snapshot_redact_multiple_sensitive_same_line() {
        assert_snapshot!(redact_sensitive("CARGO_REGISTRY_TOKEN=abc token = xyz"));
    }

    #[test]
    fn snapshot_tail_lines_zero() {
        assert_snapshot!(tail_lines("one\ntwo\nthree", 0));
    }

    #[test]
    fn snapshot_redact_case_insensitive_auth() {
        assert_snapshot!(redact_sensitive("authorization: bearer lowercasetoken"));
    }

    #[test]
    fn snapshot_redact_single_quoted_token() {
        assert_snapshot!(redact_sensitive("token = 'single_quoted_secret'"));
    }

    #[test]
    fn snapshot_tail_lines_newline_only() {
        assert_snapshot!(tail_lines("\n\n\n", 2));
    }
}
