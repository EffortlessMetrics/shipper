//! Token resolution from environment variables and credentials files.
//!
//! Returns an [`AuthInfo`] diagnostic record (token + source) rather than a
//! bare `Option<String>` so callers can render the origin for users
//! (e.g. `shipper doctor`).

use std::env;
use std::path::{Path, PathBuf};

use super::credentials::{CREDENTIALS_FILE, token_from_credentials_file};

/// Default registry name for crates.io.
pub const CRATES_IO_REGISTRY: &str = "crates-io";

/// Environment variable for the default registry token.
pub const CARGO_REGISTRY_TOKEN_ENV: &str = "CARGO_REGISTRY_TOKEN";

/// Environment variable prefix for registry-specific tokens.
pub const CARGO_REGISTRIES_TOKEN_PREFIX: &str = "CARGO_REGISTRIES_";

/// Environment variable for `CARGO_HOME`.
pub const CARGO_HOME_ENV: &str = "CARGO_HOME";

/// Result of token resolution for a single registry.
///
/// Returned by [`resolve_token`]. When `detected` is `true`, a valid
/// token was found; its origin is recorded in [`source`](AuthInfo::source).
#[derive(Debug, Clone)]
pub struct AuthInfo {
    /// The resolved token (if found).
    pub token: Option<String>,
    /// Source of the token.
    pub source: TokenSource,
    /// Whether authentication was detected.
    pub detected: bool,
}

impl Default for AuthInfo {
    fn default() -> Self {
        Self {
            token: None,
            source: TokenSource::None,
            detected: false,
        }
    }
}

/// Where the authentication token was resolved from.
///
/// Used for diagnostics and the `shipper doctor` command so users can
/// verify which credential source is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// No token found.
    None,
    /// From `CARGO_REGISTRY_TOKEN` environment variable.
    EnvDefault,
    /// From `CARGO_REGISTRIES_<NAME>_TOKEN` environment variable.
    EnvRegistry,
    /// From `$CARGO_HOME/credentials.toml`.
    CredentialsFile,
}

impl std::fmt::Display for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenSource::None => write!(f, "none"),
            TokenSource::EnvDefault => write!(f, "CARGO_REGISTRY_TOKEN"),
            TokenSource::EnvRegistry => write!(f, "CARGO_REGISTRIES_<NAME>_TOKEN"),
            TokenSource::CredentialsFile => write!(f, "credentials.toml"),
        }
    }
}

/// Resolve the authentication token for a registry.
///
/// Resolution order:
/// 1. `CARGO_REGISTRY_TOKEN` environment variable (default registry only)
/// 2. `CARGO_REGISTRIES_<NAME>_TOKEN` environment variable
/// 3. `$CARGO_HOME/credentials.toml` file
///
/// # Arguments
///
/// * `registry` — The registry name (e.g., `"crates-io"`).
/// * `cargo_home` — Optional path to `CARGO_HOME` (defaults to `$CARGO_HOME`
///   or the platform home directory's `.cargo`).
pub fn resolve_token(registry: &str, cargo_home: Option<&Path>) -> AuthInfo {
    // 1. CARGO_REGISTRY_TOKEN (for default/crates-io registry)
    if (registry == CRATES_IO_REGISTRY || registry.is_empty())
        && let Ok(token) = env::var(CARGO_REGISTRY_TOKEN_ENV)
        && !token.is_empty()
    {
        return AuthInfo {
            token: Some(token),
            source: TokenSource::EnvDefault,
            detected: true,
        };
    }

    // 2. CARGO_REGISTRIES_<NAME>_TOKEN
    let env_var = format!(
        "{}{}_TOKEN",
        CARGO_REGISTRIES_TOKEN_PREFIX,
        registry.to_uppercase().replace('-', "_")
    );
    if let Ok(token) = env::var(&env_var)
        && !token.is_empty()
    {
        return AuthInfo {
            token: Some(token),
            source: TokenSource::EnvRegistry,
            detected: true,
        };
    }

    // 3. Credentials file
    let home = cargo_home_path(cargo_home);
    let credentials_path = home.join(CREDENTIALS_FILE);

    if let Ok(token) = token_from_credentials_file(&credentials_path, registry) {
        return AuthInfo {
            token: Some(token),
            source: TokenSource::CredentialsFile,
            detected: true,
        };
    }

    AuthInfo::default()
}

/// Check whether any token is available for the given registry.
///
/// This is a convenience wrapper around [`resolve_token`] that returns
/// `true` when a token was found from any source.
pub fn has_token(registry: &str, cargo_home: Option<&Path>) -> bool {
    resolve_token(registry, cargo_home).detected
}

/// Resolve the `CARGO_HOME` directory path.
///
/// Checks, in order:
/// 1. The explicit `cargo_home` argument (if `Some`).
/// 2. The `CARGO_HOME` environment variable.
/// 3. `~/.cargo` (platform home directory).
/// 4. Falls back to `.cargo` in the current directory.
pub fn cargo_home_path(cargo_home: Option<&Path>) -> PathBuf {
    if let Some(path) = cargo_home {
        return path.to_path_buf();
    }

    if let Ok(path) = env::var(CARGO_HOME_ENV) {
        return PathBuf::from(path);
    }

    if let Some(home) = dirs::home_dir() {
        return home.join(".cargo");
    }

    PathBuf::from(".cargo")
}

/// Mask a token for safe display.
///
/// Shows the first 4 and last 4 characters, replacing the middle with
/// `****`. Tokens of 8 characters or fewer are fully masked.
pub fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}****{}", &token[..4], &token[token.len() - 4..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn mask_token_short() {
        assert_eq!(mask_token("abc"), "***");
        assert_eq!(mask_token("abcdefgh"), "********");
    }

    #[test]
    fn mask_token_long() {
        assert_eq!(mask_token("abcdefghijklmnop"), "abcd****mnop");
    }

    #[test]
    fn mask_token_empty() {
        assert_eq!(mask_token(""), "");
    }

    #[test]
    fn mask_token_boundary_nine_chars() {
        assert_eq!(mask_token("123456789"), "1234****6789");
    }

    #[test]
    fn mask_token_exactly_eight_chars() {
        assert_eq!(mask_token("12345678"), "********");
    }

    #[test]
    fn mask_token_single_char() {
        assert_eq!(mask_token("x"), "*");
    }

    #[test]
    fn cargo_home_path_uses_env() {
        let td = tempdir().expect("tempdir");
        let path = cargo_home_path(Some(td.path()));
        assert_eq!(path, td.path());
    }

    #[test]
    fn cargo_home_path_explicit_overrides_env() {
        let explicit = tempdir().expect("tempdir");
        temp_env::with_var(CARGO_HOME_ENV, Some("/some/other/path"), || {
            let path = cargo_home_path(Some(explicit.path()));
            assert_eq!(path, explicit.path());
        });
    }

    #[test]
    fn cargo_home_path_falls_back_to_env_var() {
        let td = tempdir().expect("tempdir");
        temp_env::with_var(CARGO_HOME_ENV, Some(td.path().to_str().unwrap()), || {
            let path = cargo_home_path(None);
            assert_eq!(path, td.path());
        });
    }

    #[test]
    fn cargo_home_path_no_env_falls_to_home_dir() {
        temp_env::with_var(CARGO_HOME_ENV, None::<&str>, || {
            let path = cargo_home_path(None);
            assert!(path.to_str().unwrap().contains(".cargo"));
        });
    }

    #[test]
    fn resolve_token_from_env_default() {
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("test-token"), || {
            let auth = resolve_token(CRATES_IO_REGISTRY, None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("test-token".to_string()));
            assert_eq!(auth.source, TokenSource::EnvDefault);
        });
    }

    #[test]
    fn resolve_token_from_env_registry() {
        temp_env::with_var(
            "CARGO_REGISTRIES_MY_REGISTRY_TOKEN",
            Some("custom-token"),
            || {
                let auth = resolve_token("my-registry", None);
                assert!(auth.detected);
                assert_eq!(auth.token, Some("custom-token".to_string()));
                assert_eq!(auth.source, TokenSource::EnvRegistry);
            },
        );
    }

    #[test]
    fn resolve_token_none_found() {
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<String>),
                ("CARGO_REGISTRIES_TEST_TOKEN", None::<String>),
            ],
            || {
                let auth = resolve_token("test", None);
                assert!(!auth.detected);
                assert!(auth.token.is_none());
            },
        );
    }

    #[test]
    fn token_source_display() {
        assert_eq!(TokenSource::None.to_string(), "none");
        assert_eq!(TokenSource::EnvDefault.to_string(), "CARGO_REGISTRY_TOKEN");
        assert_eq!(
            TokenSource::EnvRegistry.to_string(),
            "CARGO_REGISTRIES_<NAME>_TOKEN"
        );
        assert_eq!(TokenSource::CredentialsFile.to_string(), "credentials.toml");
    }

    #[test]
    fn auth_info_default_values() {
        let info = AuthInfo::default();
        assert!(info.token.is_none());
        assert_eq!(info.source, TokenSource::None);
        assert!(!info.detected);
    }

    #[test]
    fn resolve_token_empty_env_is_skipped() {
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, Some("")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let td = tempdir().expect("tempdir");
                let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                assert!(!auth.detected);
                assert!(auth.token.is_none());
                assert_eq!(auth.source, TokenSource::None);
            },
        );
    }

    #[test]
    fn resolve_token_empty_registry_specific_env_is_skipped() {
        temp_env::with_var("CARGO_REGISTRIES_MY_REG_TOKEN", Some(""), || {
            let td = tempdir().expect("tempdir");
            let auth = resolve_token("my-reg", Some(td.path()));
            assert!(!auth.detected);
            assert!(auth.token.is_none());
        });
    }

    #[test]
    fn resolve_token_empty_registry_name_uses_default_env() {
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("default-tok"), || {
            let auth = resolve_token("", None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("default-tok".to_string()));
            assert_eq!(auth.source, TokenSource::EnvDefault);
        });
    }

    #[test]
    fn resolve_token_custom_registry_ignores_default_env() {
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, Some("default-tok")),
                ("CARGO_REGISTRIES_CUSTOM_REG_TOKEN", None::<&str>),
            ],
            || {
                let td = tempdir().expect("tempdir");
                let auth = resolve_token("custom-reg", Some(td.path()));
                assert!(!auth.detected);
            },
        );
    }

    #[test]
    fn resolve_token_env_default_takes_priority_over_credentials() {
        let td = tempdir().expect("tempdir");
        let creds = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&creds, "[registry]\ntoken = \"creds-token\"\n").expect("write");

        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("env-token"), || {
            let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
            assert!(auth.detected);
            assert_eq!(auth.token, Some("env-token".to_string()));
            assert_eq!(auth.source, TokenSource::EnvDefault);
        });
    }

    #[test]
    fn resolve_token_env_registry_takes_priority_over_credentials() {
        let td = tempdir().expect("tempdir");
        let creds = td.path().join(CREDENTIALS_FILE);
        std::fs::write(
            &creds,
            "[registries.my-registry]\ntoken = \"creds-token\"\n",
        )
        .expect("write");

        temp_env::with_var(
            "CARGO_REGISTRIES_MY_REGISTRY_TOKEN",
            Some("env-token"),
            || {
                let auth = resolve_token("my-registry", Some(td.path()));
                assert!(auth.detected);
                assert_eq!(auth.token, Some("env-token".to_string()));
                assert_eq!(auth.source, TokenSource::EnvRegistry);
            },
        );
    }

    #[test]
    fn resolve_token_falls_through_to_credentials_file() {
        let td = tempdir().expect("tempdir");
        let creds = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&creds, "[registry]\ntoken = \"file-token\"\n").expect("write");

        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                assert!(auth.detected);
                assert_eq!(auth.token, Some("file-token".to_string()));
                assert_eq!(auth.source, TokenSource::CredentialsFile);
            },
        );
    }

    #[test]
    fn resolve_token_custom_registry_from_credentials_file() {
        let td = tempdir().expect("tempdir");
        let creds = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&creds, "[registries.private-reg]\ntoken = \"priv-token\"\n")
            .expect("write");

        temp_env::with_var("CARGO_REGISTRIES_PRIVATE_REG_TOKEN", None::<&str>, || {
            let auth = resolve_token("private-reg", Some(td.path()));
            assert!(auth.detected);
            assert_eq!(auth.token, Some("priv-token".to_string()));
            assert_eq!(auth.source, TokenSource::CredentialsFile);
        });
    }

    #[test]
    fn has_token_returns_true_when_found() {
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("tok"), || {
            assert!(has_token(CRATES_IO_REGISTRY, None));
        });
    }

    #[test]
    fn has_token_returns_false_when_missing() {
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                ("CARGO_REGISTRIES_NOEXIST_TOKEN", None::<&str>),
            ],
            || {
                let td = tempdir().expect("tempdir");
                assert!(!has_token("noexist", Some(td.path())));
            },
        );
    }

    #[test]
    fn token_source_equality() {
        assert_eq!(TokenSource::None, TokenSource::None);
        assert_eq!(TokenSource::EnvDefault, TokenSource::EnvDefault);
        assert_ne!(TokenSource::EnvDefault, TokenSource::EnvRegistry);
        assert_ne!(TokenSource::CredentialsFile, TokenSource::None);
    }

    #[test]
    fn resolve_token_registry_name_with_hyphens_maps_to_underscores() {
        temp_env::with_var(
            "CARGO_REGISTRIES_MY_CUSTOM_REG_TOKEN",
            Some("hyphen-tok"),
            || {
                let auth = resolve_token("my-custom-reg", None);
                assert!(auth.detected);
                assert_eq!(auth.token, Some("hyphen-tok".to_string()));
                assert_eq!(auth.source, TokenSource::EnvRegistry);
            },
        );
    }

    #[test]
    fn resolve_token_registry_name_uppercased() {
        temp_env::with_var("CARGO_REGISTRIES_MYREG_TOKEN", Some("upper-tok"), || {
            let auth = resolve_token("myReg", None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("upper-tok".to_string()));
            assert_eq!(auth.source, TokenSource::EnvRegistry);
        });
    }

    #[test]
    fn env_default_over_env_registry_for_crates_io() {
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, Some("env-default")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("env-registry")),
            ],
            || {
                let auth = resolve_token(CRATES_IO_REGISTRY, None);
                assert!(auth.detected);
                assert_eq!(auth.token, Some("env-default".to_string()));
                assert_eq!(auth.source, TokenSource::EnvDefault);
            },
        );
    }

    #[test]
    fn env_registry_used_when_default_unset_for_crates_io() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("env-registry")),
            ],
            || {
                let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                assert!(auth.detected);
                assert_eq!(auth.token, Some("env-registry".to_string()));
                assert_eq!(auth.source, TokenSource::EnvRegistry);
            },
        );
    }

    #[test]
    fn full_precedence_chain_env_default_wins() {
        let td = tempdir().expect("tempdir");
        let creds = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&creds, "[registry]\ntoken = \"file-token\"\n").expect("write");

        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, Some("env-default")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("env-registry")),
            ],
            || {
                let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                assert_eq!(auth.source, TokenSource::EnvDefault);
                assert_eq!(auth.token, Some("env-default".to_string()));
            },
        );
    }

    #[test]
    fn full_precedence_chain_env_registry_second() {
        let td = tempdir().expect("tempdir");
        let creds = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&creds, "[registry]\ntoken = \"file-token\"\n").expect("write");

        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("env-registry")),
            ],
            || {
                let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                assert_eq!(auth.source, TokenSource::EnvRegistry);
                assert_eq!(auth.token, Some("env-registry".to_string()));
            },
        );
    }

    #[test]
    fn full_precedence_chain_file_last() {
        let td = tempdir().expect("tempdir");
        let creds = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&creds, "[registry]\ntoken = \"file-token\"\n").expect("write");

        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                assert_eq!(auth.source, TokenSource::CredentialsFile);
                assert_eq!(auth.token, Some("file-token".to_string()));
            },
        );
    }

    #[test]
    fn resolve_token_whitespace_only_env_is_not_skipped() {
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("   "), || {
            let auth = resolve_token(CRATES_IO_REGISTRY, None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("   ".to_string()));
        });
    }

    #[test]
    fn resolve_token_very_long_env_token() {
        let long_token = "x".repeat(10_000);
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some(long_token.as_str()), || {
            let auth = resolve_token(CRATES_IO_REGISTRY, None);
            assert!(auth.detected);
            assert_eq!(auth.token.as_deref(), Some(long_token.as_str()));
        });
    }

    #[test]
    fn resolve_token_env_with_unicode() {
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("tök€n_πλ∞"), || {
            let auth = resolve_token(CRATES_IO_REGISTRY, None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("tök€n_πλ∞".to_string()));
        });
    }

    #[test]
    fn resolve_token_env_with_tabs() {
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("token\twith\ttabs"), || {
            let auth = resolve_token(CRATES_IO_REGISTRY, None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("token\twith\ttabs".to_string()));
        });
    }

    #[test]
    fn resolve_token_env_with_newlines() {
        temp_env::with_var(
            CARGO_REGISTRY_TOKEN_ENV,
            Some("token\nwith\nnewlines"),
            || {
                let auth = resolve_token(CRATES_IO_REGISTRY, None);
                assert!(auth.detected);
                assert_eq!(auth.token, Some("token\nwith\nnewlines".to_string()));
            },
        );
    }

    #[test]
    fn resolve_token_no_cargo_home_no_env_no_creds() {
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                (CARGO_HOME_ENV, None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let td = tempdir().expect("tempdir");
                let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                assert!(!auth.detected);
                assert!(auth.token.is_none());
                assert_eq!(auth.source, TokenSource::None);
            },
        );
    }

    #[test]
    fn resolve_token_multiple_registries_independent() {
        temp_env::with_vars(
            [
                (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                ("CARGO_REGISTRIES_ALPHA_TOKEN", Some("alpha-token")),
                ("CARGO_REGISTRIES_BETA_TOKEN", Some("beta-token")),
            ],
            || {
                let auth_a = resolve_token("alpha", None);
                let auth_b = resolve_token("beta", None);
                assert_eq!(auth_a.token, Some("alpha-token".to_string()));
                assert_eq!(auth_b.token, Some("beta-token".to_string()));
                assert_eq!(auth_a.source, TokenSource::EnvRegistry);
                assert_eq!(auth_b.source, TokenSource::EnvRegistry);
            },
        );
    }

    #[test]
    fn resolve_token_registry_with_numbers_in_name() {
        temp_env::with_var("CARGO_REGISTRIES_REG123_TOKEN", Some("num-tok"), || {
            let auth = resolve_token("reg123", None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("num-tok".to_string()));
        });
    }

    #[test]
    fn resolve_token_registry_single_char_name() {
        temp_env::with_var("CARGO_REGISTRIES_X_TOKEN", Some("x-tok"), || {
            let auth = resolve_token("x", None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("x-tok".to_string()));
            assert_eq!(auth.source, TokenSource::EnvRegistry);
        });
    }

    // ── snapshot tests ──────────────────────────────────────────────────

    mod snapshots {
        use super::*;
        use insta::assert_debug_snapshot;
        use tempfile::tempdir;

        #[test]
        fn snapshot_resolve_token_from_env_default() {
            temp_env::with_vars(
                [
                    (CARGO_REGISTRY_TOKEN_ENV, Some("cio-secret-token-value")),
                    ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                ],
                || {
                    let auth = resolve_token(CRATES_IO_REGISTRY, None);
                    assert_debug_snapshot!(auth);
                },
            );
        }

        #[test]
        fn snapshot_resolve_token_from_env_registry() {
            temp_env::with_vars(
                [
                    (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                    ("CARGO_REGISTRIES_MY_REGISTRY_TOKEN", Some("my-reg-token")),
                ],
                || {
                    let td = tempdir().expect("tempdir");
                    let auth = resolve_token("my-registry", Some(td.path()));
                    assert_debug_snapshot!(auth);
                },
            );
        }

        #[test]
        fn snapshot_resolve_token_none_found() {
            temp_env::with_vars(
                [
                    (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                    ("CARGO_REGISTRIES_MISSING_TOKEN", None::<&str>),
                ],
                || {
                    let td = tempdir().expect("tempdir");
                    let auth = resolve_token("missing", Some(td.path()));
                    assert_debug_snapshot!(auth);
                },
            );
        }

        #[test]
        fn snapshot_resolve_token_from_credentials_file() {
            let td = tempdir().expect("tempdir");
            let creds = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&creds, "[registry]\ntoken = \"file-secret-token\"\n").expect("write");

            temp_env::with_vars(
                [
                    (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                    ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                ],
                || {
                    let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                    assert_debug_snapshot!(auth);
                },
            );
        }

        #[test]
        fn snapshot_resolve_token_custom_registry_from_credentials() {
            let td = tempdir().expect("tempdir");
            let creds = td.path().join(CREDENTIALS_FILE);
            std::fs::write(
                &creds,
                "[registries.private-reg]\ntoken = \"priv-token-abc\"\n",
            )
            .expect("write");

            temp_env::with_var("CARGO_REGISTRIES_PRIVATE_REG_TOKEN", None::<&str>, || {
                let auth = resolve_token("private-reg", Some(td.path()));
                assert_debug_snapshot!(auth);
            });
        }

        #[test]
        fn snapshot_auth_info_default() {
            let info = AuthInfo::default();
            assert_debug_snapshot!(info);
        }
    }

    // ── edge snapshots ──────────────────────────────────────────────────

    mod edge_snapshots {
        use super::*;
        use insta::assert_debug_snapshot;

        #[test]
        fn snapshot_token_source_none() {
            assert_debug_snapshot!(TokenSource::None);
        }

        #[test]
        fn snapshot_token_source_env_default() {
            assert_debug_snapshot!(TokenSource::EnvDefault);
        }

        #[test]
        fn snapshot_token_source_env_registry() {
            assert_debug_snapshot!(TokenSource::EnvRegistry);
        }

        #[test]
        fn snapshot_token_source_credentials_file() {
            assert_debug_snapshot!(TokenSource::CredentialsFile);
        }

        #[test]
        fn snapshot_auth_info_with_env_default() {
            let info = AuthInfo {
                token: Some("tok-from-env".to_string()),
                source: TokenSource::EnvDefault,
                detected: true,
            };
            assert_debug_snapshot!(info);
        }

        #[test]
        fn snapshot_auth_info_with_env_registry() {
            let info = AuthInfo {
                token: Some("tok-from-registry-env".to_string()),
                source: TokenSource::EnvRegistry,
                detected: true,
            };
            assert_debug_snapshot!(info);
        }

        #[test]
        fn snapshot_auth_info_with_credentials_file() {
            let info = AuthInfo {
                token: Some("tok-from-file".to_string()),
                source: TokenSource::CredentialsFile,
                detected: true,
            };
            assert_debug_snapshot!(info);
        }

        #[test]
        fn snapshot_auth_info_not_detected() {
            let info = AuthInfo {
                token: None,
                source: TokenSource::None,
                detected: false,
            };
            assert_debug_snapshot!(info);
        }

        #[test]
        fn snapshot_resolve_whitespace_token() {
            temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("   "), || {
                let auth = resolve_token(CRATES_IO_REGISTRY, None);
                assert_debug_snapshot!(auth);
            });
        }

        #[test]
        fn snapshot_resolve_unicode_token() {
            temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("tök€n_πλ∞"), || {
                let auth = resolve_token(CRATES_IO_REGISTRY, None);
                assert_debug_snapshot!(auth);
            });
        }

        #[test]
        fn snapshot_mask_token_short_values() {
            let results: Vec<String> = ["", "a", "ab", "abcd", "abcdefgh"]
                .iter()
                .map(|t| mask_token(t))
                .collect();
            assert_debug_snapshot!(results);
        }

        #[test]
        fn snapshot_mask_token_long_value() {
            assert_debug_snapshot!(mask_token("abcdefghijklmnopqrstuvwxyz"));
        }
    }

    // ── error message snapshots ──────────────────────────────────────────

    mod error_message_snapshots {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn snapshot_error_missing_token_message() {
            temp_env::with_vars(
                [
                    (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                    ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                ],
                || {
                    let td = tempdir().expect("tempdir");
                    let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                    insta::assert_snapshot!(
                        "error_msg_missing_token",
                        format!(
                            "detected={}, source={}, has_token={}",
                            auth.detected,
                            auth.source,
                            auth.token.is_some()
                        )
                    );
                },
            );
        }

        #[test]
        fn snapshot_error_token_source_display_none() {
            insta::assert_snapshot!(
                "error_msg_token_source_none",
                format!("Token source: {}", TokenSource::None)
            );
        }
    }

    // ── property-based tests ─────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn token_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9_\\-\\.]{1,128}"
        }

        fn registry_name_strategy() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9\\-]{0,20}"
        }

        proptest! {
            #[test]
            fn env_default_token_resolution(token in token_strategy()) {
                temp_env::with_vars(
                    [
                        (CARGO_REGISTRY_TOKEN_ENV, Some(token.as_str())),
                        ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                    ],
                    || {
                        let auth = resolve_token(CRATES_IO_REGISTRY, None);
                        prop_assert_eq!(auth.token.as_deref(), Some(token.as_str()));
                        prop_assert_eq!(auth.source, TokenSource::EnvDefault);
                        prop_assert!(auth.detected);
                        Ok(())
                    },
                )?;
            }

            #[test]
            fn env_registry_token_resolution(
                name in registry_name_strategy(),
                token in token_strategy(),
            ) {
                let env_var = format!(
                    "{}{}_TOKEN",
                    CARGO_REGISTRIES_TOKEN_PREFIX,
                    name.to_uppercase().replace('-', "_")
                );
                temp_env::with_vars(
                    [
                        (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                        (env_var.as_str(), Some(token.as_str())),
                    ],
                    || {
                        let auth = resolve_token(&name, None);
                        prop_assert_eq!(auth.token.as_deref(), Some(token.as_str()));
                        prop_assert_eq!(auth.source, TokenSource::EnvRegistry);
                        prop_assert!(auth.detected);
                        Ok(())
                    },
                )?;
            }

            #[test]
            fn env_token_takes_precedence_over_credentials(
                env_token in token_strategy(),
                file_token in token_strategy(),
            ) {
                let td = tempfile::tempdir().expect("tempdir");
                let creds = td.path().join(CREDENTIALS_FILE);
                let content = format!("[registry]\ntoken = \"{file_token}\"\n");
                std::fs::write(&creds, &content).expect("write");

                temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some(env_token.as_str()), || {
                    let auth = resolve_token(CRATES_IO_REGISTRY, Some(td.path()));
                    prop_assert_eq!(auth.token.as_deref(), Some(env_token.as_str()));
                    prop_assert_eq!(auth.source, TokenSource::EnvDefault);
                    Ok(())
                })?;
            }

            #[test]
            fn env_registry_token_takes_precedence_over_credentials(
                name in registry_name_strategy(),
                env_token in token_strategy(),
                file_token in token_strategy(),
            ) {
                let td = tempfile::tempdir().expect("tempdir");
                let creds = td.path().join(CREDENTIALS_FILE);
                let content = format!("[registries.{name}]\ntoken = \"{file_token}\"\n");
                std::fs::write(&creds, &content).expect("write");

                let env_var = format!(
                    "{}{}_TOKEN",
                    CARGO_REGISTRIES_TOKEN_PREFIX,
                    name.to_uppercase().replace('-', "_")
                );
                temp_env::with_vars(
                    [
                        (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                        (env_var.as_str(), Some(env_token.as_str())),
                    ],
                    || {
                        let auth = resolve_token(&name, Some(td.path()));
                        prop_assert_eq!(auth.token.as_deref(), Some(env_token.as_str()));
                        prop_assert_eq!(auth.source, TokenSource::EnvRegistry);
                        Ok(())
                    },
                )?;
            }

            #[test]
            fn mask_token_never_exposes_middle(token in "[[:ascii:]]{1,200}") {
                let masked = mask_token(&token);
                if token.len() <= 8 {
                    prop_assert!(masked.chars().all(|c| c == '*'));
                    prop_assert_eq!(masked.len(), token.len());
                } else {
                    prop_assert!(masked.starts_with(&token[..4]));
                    prop_assert!(masked.ends_with(&token[token.len() - 4..]));
                    prop_assert!(masked.contains("****"));
                }
            }

            #[test]
            fn resolve_token_never_panics(registry in "[a-zA-Z0-9_\\-]{0,50}") {
                let td = tempfile::tempdir().expect("tempdir");
                temp_env::with_vars(
                    [
                        (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                        (CARGO_HOME_ENV, None::<&str>),
                    ],
                    || -> Result<(), proptest::test_runner::TestCaseError> {
                        let _ = resolve_token(&registry, Some(td.path()));
                        Ok(())
                    },
                )?;
            }

            #[test]
            fn cargo_home_path_never_panics(home in "[^\x00]{1,100}") {
                temp_env::with_var(CARGO_HOME_ENV, Some(home.as_str()), || {
                    let _ = cargo_home_path(None);
                });
            }

            #[test]
            fn mask_token_never_panics(token in "[[:ascii:]]{1,500}") {
                let _ = mask_token(&token);
            }

            #[test]
            fn has_token_never_panics(registry in "[a-zA-Z0-9_\\-]{0,50}") {
                let td = tempfile::tempdir().expect("tempdir");
                temp_env::with_vars(
                    [
                        (CARGO_REGISTRY_TOKEN_ENV, None::<&str>),
                        (CARGO_HOME_ENV, None::<&str>),
                    ],
                    || -> Result<(), proptest::test_runner::TestCaseError> {
                        let _ = has_token(&registry, Some(td.path()));
                        Ok(())
                    },
                )?;
            }
        }
    }
}
