//! GitHub Actions trusted-publishing (OIDC) detection.
//!
//! Trusted publishing exchanges an OIDC identity token for a short-lived
//! registry token. Detection is a simple env-var check; the actual exchange is
//! performed later in the pipeline. Both variables must contain non-blank
//! values to return `true`.

use std::env;

/// Detect whether trusted publishing (GitHub Actions OIDC) is available.
///
/// Returns `true` when both `ACTIONS_ID_TOKEN_REQUEST_URL` and
/// `ACTIONS_ID_TOKEN_REQUEST_TOKEN` environment variables contain non-blank
/// values, indicating a GitHub Actions environment with OIDC token support.
pub fn is_trusted_publishing_available() -> bool {
    has_nonblank_value("ACTIONS_ID_TOKEN_REQUEST_URL")
        && has_nonblank_value("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
}

pub(crate) fn has_nonblank_value(name: &str) -> bool {
    env::var(name).is_ok_and(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn trusted_publishing_both_vars_set() {
        temp_env::with_vars(
            [
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("oidc-token")),
            ],
            || {
                assert!(is_trusted_publishing_available());
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_only_url_set() {
        temp_env::with_vars(
            [
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
            ],
            || {
                assert!(!is_trusted_publishing_available());
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_only_token_set() {
        temp_env::with_vars(
            [
                ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("oidc-token")),
            ],
            || {
                assert!(!is_trusted_publishing_available());
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_neither_set() {
        temp_env::with_vars(
            [
                ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
            ],
            || {
                assert!(!is_trusted_publishing_available());
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_empty_values_are_unavailable() {
        temp_env::with_vars(
            [
                ("ACTIONS_ID_TOKEN_REQUEST_URL", Some("")),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("")),
            ],
            || {
                assert!(!is_trusted_publishing_available());
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_whitespace_values_are_unavailable() {
        temp_env::with_vars(
            [
                ("ACTIONS_ID_TOKEN_REQUEST_URL", Some("  \t")),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("\n ")),
            ],
            || {
                assert!(!is_trusted_publishing_available());
            },
        );
    }

    #[test]
    #[serial]
    fn trusted_publishing_one_blank_value_is_unavailable() {
        temp_env::with_vars(
            [
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("")),
            ],
            || {
                assert!(!is_trusted_publishing_available());
            },
        );
    }
}
