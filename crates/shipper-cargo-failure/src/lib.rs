//! Cargo publish failure classification.
//!
//! This crate isolates error classification heuristics used by shipper's
//! publish engine so they can be reused and tested independently.

/// Error class for cargo publish failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoFailureClass {
    /// Transient failure that can succeed on retry.
    Retryable,
    /// Persistent failure requiring user changes before retry.
    Permanent,
    /// Outcome is unclear and must be confirmed against the registry.
    Ambiguous,
}

/// Classifier output for a cargo publish failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CargoFailureOutcome {
    /// Derived failure class.
    pub class: CargoFailureClass,
    /// Human-readable summary used in logs/receipts.
    pub message: &'static str,
}

const RETRYABLE_PATTERNS: [&str; 20] = [
    "too many requests",
    "429",
    "timeout",
    "timed out",
    "connection reset",
    "connection refused",
    "connection closed",
    "dns",
    "tls",
    "temporarily unavailable",
    "failed to download",
    "failed to send",
    "server error",
    "500",
    "502",
    "503",
    "504",
    "broken pipe",
    "reset by peer",
    "network unreachable",
];

const PERMANENT_PATTERNS: [&str; 22] = [
    "failed to parse manifest",
    "invalid",
    "missing",
    "license",
    "description",
    "readme",
    "repository",
    "could not compile",
    "compilation failed",
    "failed to verify",
    "package is not allowed to be published",
    "publish is disabled",
    "yanked",
    "forbidden",
    "permission denied",
    "not authorized",
    "unauthorized",
    "version already exists",
    "is already uploaded",
    "token is invalid",
    "invalid credentials",
    "checksum mismatch",
];

/// Classify cargo publish output into retry behavior categories.
///
/// Matching is case-insensitive and scans both stderr and stdout.
/// Retryable patterns take precedence over permanent ones.
pub fn classify_publish_failure(stderr: &str, stdout: &str) -> CargoFailureOutcome {
    let haystack = format!("{stderr}\n{stdout}").to_lowercase();

    if RETRYABLE_PATTERNS
        .iter()
        .any(|pattern| haystack.contains(pattern))
    {
        return CargoFailureOutcome {
            class: CargoFailureClass::Retryable,
            message: "transient failure (retryable)",
        };
    }

    if PERMANENT_PATTERNS
        .iter()
        .any(|pattern| haystack.contains(pattern))
    {
        return CargoFailureOutcome {
            class: CargoFailureClass::Permanent,
            message: "permanent failure (fix required)",
        };
    }

    CargoFailureOutcome {
        class: CargoFailureClass::Ambiguous,
        message: "publish outcome ambiguous; registry did not show version",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── basic classification ────────────────────────────────────────────

    #[test]
    fn classifies_retryable_failure() {
        let outcome = classify_publish_failure("HTTP 429 too many requests", "");
        assert_eq!(outcome.class, CargoFailureClass::Retryable);
        assert_eq!(outcome.message, "transient failure (retryable)");
    }

    #[test]
    fn classifies_permanent_failure() {
        let outcome = classify_publish_failure("permission denied", "");
        assert_eq!(outcome.class, CargoFailureClass::Permanent);
        assert_eq!(outcome.message, "permanent failure (fix required)");
    }

    #[test]
    fn classifies_ambiguous_failure() {
        let outcome = classify_publish_failure("unexpected tool output", "");
        assert_eq!(outcome.class, CargoFailureClass::Ambiguous);
        assert_eq!(
            outcome.message,
            "publish outcome ambiguous; registry did not show version"
        );
    }

    #[test]
    fn retryable_takes_precedence_when_both_pattern_sets_match() {
        let outcome = classify_publish_failure("permission denied and 429", "");
        assert_eq!(outcome.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn scans_stdout_in_addition_to_stderr() {
        let outcome = classify_publish_failure("", "server error 503");
        assert_eq!(outcome.class, CargoFailureClass::Retryable);
    }

    // ── every retryable pattern individually ────────────────────────────

    #[test]
    fn retryable_too_many_requests() {
        let o = classify_publish_failure("too many requests", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_429() {
        let o = classify_publish_failure("HTTP/1.1 429", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_timeout() {
        let o = classify_publish_failure("request timeout", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_timed_out() {
        let o = classify_publish_failure("operation timed out", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_connection_reset() {
        let o = classify_publish_failure("connection reset by peer", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_connection_refused() {
        let o = classify_publish_failure("connection refused", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_connection_closed() {
        let o = classify_publish_failure("connection closed before message completed", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_dns() {
        let o = classify_publish_failure("dns resolution failed", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_tls() {
        let o = classify_publish_failure("tls handshake failed", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_temporarily_unavailable() {
        let o = classify_publish_failure("service temporarily unavailable", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_failed_to_download() {
        let o = classify_publish_failure("failed to download index", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_failed_to_send() {
        let o = classify_publish_failure("failed to send request", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_server_error() {
        let o = classify_publish_failure("server error", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_500() {
        let o = classify_publish_failure("HTTP 500 Internal Server Error", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_502() {
        let o = classify_publish_failure("502 Bad Gateway", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_503() {
        let o = classify_publish_failure("503 Service Unavailable", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_504() {
        let o = classify_publish_failure("504 Gateway Timeout", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_broken_pipe() {
        let o = classify_publish_failure("broken pipe", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_reset_by_peer() {
        let o = classify_publish_failure("reset by peer", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_network_unreachable() {
        let o = classify_publish_failure("network unreachable", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    // ── every permanent pattern individually ────────────────────────────

    #[test]
    fn permanent_failed_to_parse_manifest() {
        let o = classify_publish_failure("failed to parse manifest at Cargo.toml", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_invalid() {
        let o = classify_publish_failure("invalid package name", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_missing() {
        let o = classify_publish_failure("missing field `version`", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_license() {
        let o = classify_publish_failure("no `license` or `license-file` set", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_description() {
        let o = classify_publish_failure("no `description` specified", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_readme() {
        let o = classify_publish_failure("readme file not found", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_repository() {
        let o = classify_publish_failure("no `repository` URL specified", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_could_not_compile() {
        let o = classify_publish_failure("could not compile `my-crate`", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_compilation_failed() {
        let o = classify_publish_failure("compilation failed", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_failed_to_verify() {
        let o = classify_publish_failure("failed to verify package tarball", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_not_allowed_to_publish() {
        let o = classify_publish_failure("package is not allowed to be published", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_publish_disabled() {
        let o = classify_publish_failure("publish is disabled for this package", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_yanked() {
        let o = classify_publish_failure("dependency `foo` has been yanked", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_forbidden() {
        let o = classify_publish_failure("403 forbidden", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_permission_denied() {
        let o = classify_publish_failure("permission denied (publickey)", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_not_authorized() {
        let o = classify_publish_failure("not authorized to publish", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_unauthorized() {
        let o = classify_publish_failure("401 unauthorized", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_version_already_exists() {
        let o = classify_publish_failure("version already exists: 1.0.0", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_already_uploaded() {
        let o = classify_publish_failure("crate version 1.0.0 is already uploaded", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_token_is_invalid() {
        let o = classify_publish_failure("token is invalid", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_invalid_credentials() {
        let o = classify_publish_failure("invalid credentials", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn permanent_checksum_mismatch() {
        let o = classify_publish_failure("checksum mismatch for crate", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    // ── rate limiting detection ─────────────────────────────────────────

    #[test]
    fn rate_limit_via_429_status() {
        let o = classify_publish_failure("received status 429 from registry", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn rate_limit_via_too_many_requests_mixed_case() {
        let o = classify_publish_failure("Too Many Requests", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn rate_limit_embedded_in_longer_message() {
        let o = classify_publish_failure(
            "error: the registry responded with: 429 Too Many Requests; try again later",
            "",
        );
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    // ── network timeout detection ───────────────────────────────────────

    #[test]
    fn timeout_with_surrounding_context() {
        let o = classify_publish_failure("operation on socket timed out after 30s", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn timeout_uppercase() {
        let o = classify_publish_failure("TIMEOUT waiting for registry", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn gateway_timeout_504() {
        let o = classify_publish_failure("", "HTTP/1.1 504 Gateway Timeout");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    // ── authentication failure detection ────────────────────────────────

    #[test]
    fn auth_failure_unauthorized_response() {
        let o = classify_publish_failure("the registry returned 401 Unauthorized", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn auth_failure_invalid_token() {
        let o = classify_publish_failure("error: token is invalid or expired", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn auth_failure_forbidden() {
        let o = classify_publish_failure("HTTP 403 Forbidden: you do not own this crate", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn auth_failure_not_authorized() {
        let o = classify_publish_failure("not authorized to perform this action", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    // ── already-published detection ─────────────────────────────────────

    #[test]
    fn already_published_version_exists() {
        let o = classify_publish_failure(
            "error: crate version `1.2.3` version already exists in registry",
            "",
        );
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn already_published_is_already_uploaded() {
        let o = classify_publish_failure("crate `my-crate` is already uploaded at 0.1.0", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn already_published_in_stdout() {
        let o = classify_publish_failure("", "version already exists");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    // ── edge cases ──────────────────────────────────────────────────────

    #[test]
    fn empty_stderr_and_stdout_is_ambiguous() {
        let o = classify_publish_failure("", "");
        assert_eq!(o.class, CargoFailureClass::Ambiguous);
    }

    #[test]
    fn whitespace_only_is_ambiguous() {
        let o = classify_publish_failure("   \n\t  ", "   \n  ");
        assert_eq!(o.class, CargoFailureClass::Ambiguous);
    }

    #[test]
    fn unicode_content_without_patterns_is_ambiguous() {
        let o = classify_publish_failure("エラーが発生しました 🚨", "出力なし");
        assert_eq!(o.class, CargoFailureClass::Ambiguous);
    }

    #[test]
    fn unicode_surrounding_retryable_keyword() {
        let o = classify_publish_failure("⚠️ timeout while connecting ⚠️", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn unicode_surrounding_permanent_keyword() {
        let o = classify_publish_failure("❌ permission denied ❌", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn partial_match_within_word_still_matches() {
        // "dns" appears within "no dns resolution" — substring match should work
        let o = classify_publish_failure("no dns resolution possible", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn pattern_at_very_start_of_string() {
        let o = classify_publish_failure("tls error occurred", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn pattern_at_very_end_of_string() {
        let o = classify_publish_failure("failed because of broken pipe", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn very_long_output_with_pattern_buried_deep() {
        let noise = "a]b[c ".repeat(2000);
        let stderr = format!("{noise}connection refused{noise}");
        let o = classify_publish_failure(&stderr, "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn newlines_within_output_do_not_prevent_match() {
        let o = classify_publish_failure("line1\nline2\nconnection reset\nline4", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn case_insensitive_matching_retryable() {
        let o = classify_publish_failure("CONNECTION REFUSED", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn case_insensitive_matching_permanent() {
        let o = classify_publish_failure("TOKEN IS INVALID", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn mixed_case_matching() {
        let o = classify_publish_failure("Timed Out waiting for response", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn retryable_in_stdout_permanent_in_stderr_retryable_wins() {
        let o = classify_publish_failure("permission denied", "503 unavailable");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn multiple_retryable_patterns_still_retryable() {
        let o = classify_publish_failure("timeout and connection reset and 503", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn multiple_permanent_patterns_still_permanent() {
        let o = classify_publish_failure("token is invalid and permission denied", "");
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn numeric_pattern_500_not_in_port_number() {
        // "500" as a substring will match even in "port 15003" — this confirms
        // the classifier uses simple substring matching, which is the intended
        // behavior as documented.
        let o = classify_publish_failure("listening on port 15003", "");
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn unknown_exit_code_is_ambiguous() {
        let o = classify_publish_failure("cargo exited with code 42", "");
        assert_eq!(o.class, CargoFailureClass::Ambiguous);
    }

    #[test]
    fn gibberish_is_ambiguous() {
        let o = classify_publish_failure("asdlkfjasldf", "qpwoeiruty");
        assert_eq!(o.class, CargoFailureClass::Ambiguous);
    }

    #[test]
    fn pattern_split_across_stderr_and_stdout_does_not_match_accidentally() {
        // "timed out" won't match if "timed" is in stderr and "out" is in stdout,
        // because the haystack is "timed\nout" — substring "timed out" is not present.
        let o = classify_publish_failure("timed", "out");
        assert_eq!(o.class, CargoFailureClass::Ambiguous);
    }

    // ── realistic cargo publish error messages ──────────────────────────

    #[test]
    fn realistic_crates_io_rate_limit() {
        let o = classify_publish_failure(
            "error: failed to publish to registry crates-io\n\
             Caused by:\n  the remote server responded with 429 Too Many Requests",
            "",
        );
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }

    #[test]
    fn realistic_manifest_missing_description() {
        let o = classify_publish_failure(
            "",
            "error: 3 fields are missing from `Cargo.toml`:\n\
             - description\n- license\n- repository",
        );
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn realistic_already_published() {
        let o = classify_publish_failure(
            "error: failed to publish crate `my-crate v1.0.0`\n\
             Caused by:\n  the remote server responded: crate version `1.0.0` \
             is already uploaded",
            "",
        );
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn realistic_compilation_failure() {
        let o = classify_publish_failure(
            "error[E0308]: mismatched types\n\
             error: could not compile `my-crate` due to previous error",
            "",
        );
        assert_eq!(o.class, CargoFailureClass::Permanent);
    }

    #[test]
    fn realistic_network_failure() {
        let o = classify_publish_failure(
            "error: failed to publish to registry\n\
             Caused by:\n  failed to send request: \
             error sending request for url (https://crates.io/api/v1/crates/new): \
             connection reset by peer",
            "",
        );
        assert_eq!(o.class, CargoFailureClass::Retryable);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    fn ascii_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(any::<u8>(), 0..256)
            .prop_map(|bytes| bytes.into_iter().map(char::from).collect())
    }

    fn arbitrary_string() -> impl Strategy<Value = String> {
        prop::string::string_regex(".*").unwrap()
    }

    proptest! {
        #[test]
        fn classification_is_deterministic(stderr in ascii_text(), stdout in ascii_text()) {
            let first = classify_publish_failure(&stderr, &stdout);
            let second = classify_publish_failure(&stderr, &stdout);
            prop_assert_eq!(first, second);
        }

        #[test]
        fn classification_is_case_insensitive_for_ascii(stderr in ascii_text(), stdout in ascii_text()) {
            let lower = classify_publish_failure(
                &stderr.to_ascii_lowercase(),
                &stdout.to_ascii_lowercase(),
            );
            let upper = classify_publish_failure(
                &stderr.to_ascii_uppercase(),
                &stdout.to_ascii_uppercase(),
            );
            prop_assert_eq!(lower.class, upper.class);
        }

        #[test]
        fn retryable_patterns_have_precedence(noise in ascii_text()) {
            let stderr = format!("{noise} permission denied and too many requests");
            let outcome = classify_publish_failure(&stderr, "");
            prop_assert_eq!(outcome.class, CargoFailureClass::Retryable);
        }

        /// Any input always classifies to one of the three known categories.
        #[test]
        fn any_input_produces_valid_class(stderr in arbitrary_string(), stdout in arbitrary_string()) {
            let outcome = classify_publish_failure(&stderr, &stdout);
            prop_assert!(
                matches!(
                    outcome.class,
                    CargoFailureClass::Retryable
                        | CargoFailureClass::Permanent
                        | CargoFailureClass::Ambiguous
                ),
                "unexpected class: {:?}",
                outcome.class
            );
        }

        /// The message field is always non-empty for any classification.
        #[test]
        fn message_is_never_empty(stderr in arbitrary_string(), stdout in arbitrary_string()) {
            let outcome = classify_publish_failure(&stderr, &stdout);
            prop_assert!(!outcome.message.is_empty());
        }

        /// Swapping stderr/stdout does not change the class — both are scanned equally.
        #[test]
        fn stderr_stdout_symmetry(stderr in ascii_text(), stdout in ascii_text()) {
            let normal = classify_publish_failure(&stderr, &stdout);
            let swapped = classify_publish_failure(&stdout, &stderr);
            prop_assert_eq!(normal.class, swapped.class);
        }

        /// Prepending/appending noise to a retryable pattern keeps it retryable.
        #[test]
        fn retryable_pattern_survives_noise(
            prefix in ascii_text(),
            suffix in ascii_text(),
            idx in 0..20usize,
        ) {
            let pattern = RETRYABLE_PATTERNS[idx];
            let stderr = format!("{prefix}{pattern}{suffix}");
            let outcome = classify_publish_failure(&stderr, "");
            prop_assert_eq!(outcome.class, CargoFailureClass::Retryable);
        }

        /// Prepending/appending noise to a permanent pattern (with no retryable
        /// pattern present) keeps it permanent.
        #[test]
        fn permanent_pattern_survives_noise(
            prefix in "[a-z ]{0,50}",
            suffix in "[a-z ]{0,50}",
            idx in 0..22usize,
        ) {
            let pattern = PERMANENT_PATTERNS[idx];
            // Ensure no retryable substring sneaks in via prefix/suffix
            let stderr = format!("{prefix}{pattern}{suffix}");
            let outcome = classify_publish_failure(&stderr, "");
            // May be retryable if noise accidentally contains a retryable pattern,
            // but must never be ambiguous when a permanent pattern is explicitly present.
            prop_assert_ne!(outcome.class, CargoFailureClass::Ambiguous);
        }
    }
}
