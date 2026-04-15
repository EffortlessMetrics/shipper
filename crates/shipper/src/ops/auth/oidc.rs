//! GitHub Actions trusted-publishing (OIDC) detection.
//!
//! Trusted publishing exchanges an OIDC identity token for a short-lived
//! registry token. Detection is a simple env-var presence check; the actual
//! exchange is performed later in the pipeline. Both variables must be set
//! to return `true`.

use std::env;

/// Detect whether trusted publishing (GitHub Actions OIDC) is available.
///
/// Returns `true` when both `ACTIONS_ID_TOKEN_REQUEST_URL` and
/// `ACTIONS_ID_TOKEN_REQUEST_TOKEN` environment variables are set,
/// indicating a GitHub Actions environment with OIDC token support.
pub fn is_trusted_publishing_available() -> bool {
    env::var_os("ACTIONS_ID_TOKEN_REQUEST_URL").is_some()
        && env::var_os("ACTIONS_ID_TOKEN_REQUEST_TOKEN").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
    fn trusted_publishing_empty_values_still_detected() {
        // var_os returns Some("") for empty values — still "set"
        temp_env::with_vars(
            [
                ("ACTIONS_ID_TOKEN_REQUEST_URL", Some("")),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("")),
            ],
            || {
                assert!(is_trusted_publishing_available());
            },
        );
    }
}
