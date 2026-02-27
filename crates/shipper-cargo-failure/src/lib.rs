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
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    fn ascii_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(any::<u8>(), 0..256)
            .prop_map(|bytes| bytes.into_iter().map(char::from).collect())
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
    }
}
