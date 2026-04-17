//! Output sanitization helpers for cargo command logs and evidence payloads.

/// Strip ANSI escape sequences (CSI/OSC/etc.) from a string.
///
/// Cargo colorizes its output with SGR codes like `\x1b[1m` (bold) and
/// `\x1b[92m` (bright green). Those bytes are noise in anything that will
/// be parsed or rendered outside a terminal — events, receipts, sidecar
/// files, dashboards. This function removes every `\x1b[...]` CSI sequence
/// plus `\x1b]...\x07` OSC sequences and bare `\x1b` characters, leaving
/// the underlying text intact.
///
/// Dependency-free and allocation-frugal — processes one byte at a time.
///
/// # Examples
///
/// ```
/// use shipper_output_sanitizer::strip_ansi;
///
/// let colored = "\x1b[1m\x1b[92m   Compiling\x1b[0m demo v0.1.0";
/// assert_eq!(strip_ansi(colored), "   Compiling demo v0.1.0");
///
/// // No-op on plain strings.
/// assert_eq!(strip_ansi("hello"), "hello");
/// ```
pub fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() {
            match bytes[i + 1] {
                // CSI: \x1b[ ... <final byte 0x40..=0x7e>
                b'[' => {
                    i += 2;
                    while i < bytes.len() {
                        let b = bytes[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&b) {
                            break;
                        }
                    }
                }
                // OSC: \x1b] ... BEL (0x07) or ST (\x1b\\)
                b']' => {
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                // Two-byte escape (e.g. \x1b(B, \x1b=, …) — skip next byte.
                _ => i += 2,
            }
        } else {
            // Non-ESC byte — append as UTF-8-safe codepoint.
            let ch = s[i..].chars().next().unwrap_or('\0');
            let len = ch.len_utf8();
            out.push(ch);
            i += len;
        }
    }
    out
}

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
    // Normalize line endings before splitting to ensure consistent line counts.
    let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let tail = if lines.len() <= n {
        normalized
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
    // Normalize line endings to \n for idempotence (\r\n → \n, bare \r → \n).
    let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
    let mut result = String::with_capacity(normalized.len());
    let mut first = true;
    for line in normalized.lines() {
        if !first {
            result.push('\n');
        }
        first = false;
        result.push_str(&redact_line(line));
    }
    // Preserve trailing newline if present.
    if normalized.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod strip_ansi_tests {
    use super::strip_ansi;

    #[test]
    fn strips_sgr_color_codes() {
        let input = "\x1b[1m\x1b[92m   Compiling\x1b[0m demo v0.1.0";
        assert_eq!(strip_ansi(input), "   Compiling demo v0.1.0");
    }

    #[test]
    fn strips_multiple_codes_and_preserves_newlines() {
        let input = "\x1b[31merror\x1b[0m: thing\n\x1b[33mwarning\x1b[0m: other\n";
        assert_eq!(strip_ansi(input), "error: thing\nwarning: other\n");
    }

    #[test]
    fn noop_on_plain_strings() {
        assert_eq!(strip_ansi("hello"), "hello");
        assert_eq!(strip_ansi(""), "");
        assert_eq!(strip_ansi("line1\nline2"), "line1\nline2");
    }

    #[test]
    fn strips_cargo_style_dry_run_output() {
        let input = "\x1b[1m\x1b[92m   Compiling\x1b[0m anstyle v1.0.14\n\x1b[1m\x1b[33mwarning\x1b[0m: aborting upload due to dry run\n";
        let out = strip_ansi(input);
        assert!(
            !out.contains('\x1b'),
            "no ESC bytes should remain: {:?}",
            out
        );
        assert!(out.contains("Compiling"));
        assert!(out.contains("warning"));
        assert!(out.contains("aborting upload"));
    }

    #[test]
    fn handles_utf8_between_escapes() {
        let input = "\x1b[1mhello, 世界\x1b[0m";
        assert_eq!(strip_ansi(input), "hello, 世界");
    }

    #[test]
    fn strips_osc_sequences() {
        let input = "\x1b]0;title\x07done";
        assert_eq!(strip_ansi(input), "done");
    }
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

    // --- Token pattern tests ---

    #[test]
    fn redact_bearer_jwt_like_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.rXz";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_basic_auth_not_touched() {
        // Only bearer tokens are redacted; Basic auth passes through
        let input = "Authorization: Basic dXNlcjpwYXNz";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization: Basic dXNlcjpwYXNz");
    }

    #[test]
    fn redact_multiple_bearer_across_lines() {
        let input = "Authorization: Bearer tok1\nOther line\nAuthorization: Bearer tok2";
        let out = redact_sensitive(input);
        assert_eq!(
            out,
            "Authorization: Bearer [REDACTED]\nOther line\nAuthorization: Bearer [REDACTED]"
        );
    }

    #[test]
    fn redact_multiple_registries_tokens_multiline() {
        let input = "CARGO_REGISTRIES_STAGING_TOKEN=stg\nCARGO_REGISTRIES_PROD_TOKEN=prd";
        let out = redact_sensitive(input);
        assert_eq!(
            out,
            "CARGO_REGISTRIES_STAGING_TOKEN=[REDACTED]\nCARGO_REGISTRIES_PROD_TOKEN=[REDACTED]"
        );
    }

    #[test]
    fn redact_token_in_url_query_param() {
        let input = "https://crates.io/api?token=secret_api_key";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("secret_api_key"));
    }

    #[test]
    fn redact_bearer_with_extra_whitespace() {
        let input = "Authorization:   Bearer   tok123";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization:   Bearer [REDACTED]");
    }

    #[test]
    fn redact_credentials_toml_format() {
        let input = "[registries.my-reg]\ntoken = \"cio_secret\"";
        let out = redact_sensitive(input);
        assert!(out.contains("[registries.my-reg]"));
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("cio_secret"));
    }

    // --- No false positives ---

    #[test]
    fn no_false_positive_windows_path() {
        let input = r"C:\Users\admin\.cargo\registry\cache\crate-0.1.0";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn no_false_positive_unix_path() {
        let input = "/home/user/.cargo/registry/src/index.crates.io-1ecc6299db9ec823";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn no_false_positive_cargo_home_env() {
        let input = "CARGO_HOME=/home/user/.cargo";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn no_false_positive_temp_path() {
        let input = "/tmp/cargo-installXXXXXX/release/mycrate";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn no_false_positive_tokenize_word() {
        let input = "We tokenize the input and parse it.";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn no_false_positive_token_in_prose_no_equals() {
        let input = "Please provide your authentication token via the CLI.";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn no_false_positive_normal_cargo_compile_output() {
        let input = "   Compiling serde v1.0.200\n   Compiling tokio v1.37.0\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.34s";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    // --- Mixed content ---

    #[test]
    fn mixed_token_and_cargo_output() {
        let input =
            "   Compiling mycrate v0.1.0\nAuthorization: Bearer secret123\n   Finished release";
        let out = redact_sensitive(input);
        assert!(out.contains("Compiling mycrate v0.1.0"));
        assert!(out.contains("Bearer [REDACTED]"));
        assert!(out.contains("Finished release"));
        assert!(!out.contains("secret123"));
    }

    #[test]
    fn mixed_env_vars_sensitive_and_benign() {
        let input = "CARGO_HOME=/usr/local/cargo\nCARGO_REGISTRY_TOKEN=secret";
        let out = redact_sensitive(input);
        assert!(out.contains("CARGO_HOME=/usr/local/cargo"));
        assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(!out.contains("=secret"));
    }

    // --- Unicode ---

    #[test]
    fn unicode_cjk_not_corrupted() {
        let input = "ビルド成功: mycrate v0.1.0";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn unicode_emoji_preserved() {
        let input = "✅ Published successfully! 🎉 crate uploaded.";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn unicode_with_token_redaction() {
        let input = "🔑 CARGO_REGISTRY_TOKEN=secret123";
        let out = redact_sensitive(input);
        assert!(out.contains("🔑"));
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("secret123"));
    }

    // --- Large output ---

    #[test]
    fn large_output_many_lines() {
        let mut lines: Vec<String> = (0..10_000)
            .map(|i| format!("Compiling crate_{i} v0.1.0"))
            .collect();
        lines[5000] = "CARGO_REGISTRY_TOKEN=hidden_secret".to_string();
        let input = lines.join("\n");
        let out = redact_sensitive(&input);
        assert!(!out.contains("hidden_secret"));
        assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(out.contains("Compiling crate_0 v0.1.0"));
        assert!(out.contains("Compiling crate_9999 v0.1.0"));
        assert_eq!(out.lines().count(), 10_000);
    }

    #[test]
    fn large_single_line_with_token() {
        let prefix = "x".repeat(100_000);
        let input = format!("{prefix} CARGO_REGISTRY_TOKEN=longsecret");
        let out = redact_sensitive(&input);
        assert!(!out.contains("longsecret"));
        assert!(out.contains("[REDACTED]"));
    }

    // --- tail_lines additional ---

    #[test]
    fn tail_lines_redacts_all_sensitive_in_window() {
        let input = "safe\nAuthorization: Bearer tok1\ntoken = secret2\nlast";
        let out = tail_lines(input, 3);
        assert_eq!(
            out,
            "Authorization: Bearer [REDACTED]\ntoken = [REDACTED]\nlast"
        );
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

            let expected = expected_tail
                .iter()
                .map(|line| redact_line(line))
                .collect::<Vec<String>>()
                .join("\n");
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

        #[test]
        fn cargo_registry_token_always_redacted(secret in "[a-z0-9]{1,30}") {
            let input = format!("CARGO_REGISTRY_TOKEN={secret}");
            let out = redact_sensitive(&input);
            prop_assert!(!out.contains(&*secret), "Secret '{}' leaked in: {}", secret, out);
            prop_assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
        }

        #[test]
        fn token_assignment_always_redacted(secret in "[0-9]{3,20}") {
            let input = format!("token = {secret}");
            let out = redact_sensitive(&input);
            prop_assert!(out.contains("[REDACTED]"), "Expected [REDACTED] in: {}", out);
            prop_assert!(!out.contains(&*secret), "Secret '{}' leaked in: {}", secret, out);
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

    #[test]
    fn snapshot_multiline_mixed_token_types() {
        let input = "Compiling foo v1.0\nAuthorization: Bearer jwt_tok_123\nCARGO_REGISTRY_TOKEN=cio_abc\ntoken = \"mysecret\"\nDone publishing";
        assert_snapshot!(redact_sensitive(input));
    }

    #[test]
    fn snapshot_unicode_with_redaction() {
        let input =
            "🚀 Déploiement: mycrate v0.1.0\n🔑 CARGO_REGISTRY_TOKEN=secret_val\n✅ Terminé!";
        assert_snapshot!(redact_sensitive(input));
    }

    #[test]
    fn snapshot_typical_cargo_publish_output() {
        let input = "   Compiling mycrate v0.2.0 (/home/user/project)\n    Finished `release` profile [optimized] target(s) in 3.42s\n   Uploading mycrate v0.2.0 (/home/user/project/Cargo.toml)\n    Uploaded mycrate v0.2.0 to `crates.io`\nnote: waiting for `mycrate v0.2.0` to be available at registry `crates.io`\npublished mycrate v0.2.0 at registry `crates.io`";
        assert_snapshot!(redact_sensitive(input));
    }
}
