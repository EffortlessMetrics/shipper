//! Authentication and token resolution for shipper.
//!
//! This crate provides token resolution from multiple sources:
//! - Environment variables (`CARGO_REGISTRY_TOKEN`, `CARGO_REGISTRIES_<NAME>_TOKEN`)
//! - Cargo credentials file (`$CARGO_HOME/credentials.toml`)
//!
//! # Example
//!
//! ```
//! use shipper_auth::resolve_token;
//! use std::path::Path;
//!
//! // Resolve token for crates.io
//! let token = resolve_token("crates-io", None);
//!
//! // Resolve token for a custom registry
//! let token = resolve_token("my-registry", Some(Path::new("/path/to/cargo/home")));
//! ```

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Default registry name for crates.io
pub const CRATES_IO_REGISTRY: &str = "crates-io";

/// Environment variable for the default registry token
pub const CARGO_REGISTRY_TOKEN_ENV: &str = "CARGO_REGISTRY_TOKEN";

/// Environment variable prefix for registry-specific tokens
pub const CARGO_REGISTRIES_TOKEN_PREFIX: &str = "CARGO_REGISTRIES_";

/// Environment variable for CARGO_HOME
pub const CARGO_HOME_ENV: &str = "CARGO_HOME";

/// Credentials file name
pub const CREDENTIALS_FILE: &str = "credentials.toml";

/// Authentication information
#[derive(Debug, Clone)]
pub struct AuthInfo {
    /// The resolved token (if found)
    pub token: Option<String>,
    /// Source of the token
    pub source: TokenSource,
    /// Whether authentication was detected
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

/// Source of the authentication token
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// No token found
    None,
    /// From `CARGO_REGISTRY_TOKEN` environment variable
    EnvDefault,
    /// From `CARGO_REGISTRIES_<NAME>_TOKEN` environment variable
    EnvRegistry,
    /// From `$CARGO_HOME/credentials.toml`
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
/// This checks in order:
/// 1. `CARGO_REGISTRY_TOKEN` environment variable (for default registry)
/// 2. `CARGO_REGISTRIES_<NAME>_TOKEN` environment variable
/// 3. `$CARGO_HOME/credentials.toml` file
///
/// # Arguments
///
/// * `registry` - The registry name (e.g., "crates-io")
/// * `cargo_home` - Optional path to CARGO_HOME (defaults to `$CARGO_HOME` or `~/.cargo`)
///
/// # Returns
///
/// The authentication information, including token and source.
pub fn resolve_token(registry: &str, cargo_home: Option<&Path>) -> AuthInfo {
    // 1. Check CARGO_REGISTRY_TOKEN (for default/crates-io registry)
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

    // 2. Check CARGO_REGISTRIES_<NAME>_TOKEN
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

    // 3. Check credentials file
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

/// Check if a token is available for the registry.
pub fn has_token(registry: &str, cargo_home: Option<&Path>) -> bool {
    resolve_token(registry, cargo_home).detected
}

/// Get the default CARGO_HOME path.
pub fn cargo_home_path(cargo_home: Option<&Path>) -> PathBuf {
    if let Some(path) = cargo_home {
        return path.to_path_buf();
    }

    if let Ok(path) = env::var(CARGO_HOME_ENV) {
        return PathBuf::from(path);
    }

    // Default to ~/.cargo
    if let Some(home) = dirs::home_dir() {
        return home.join(".cargo");
    }

    PathBuf::from(".cargo")
}

/// Read token from credentials.toml file.
fn token_from_credentials_file(path: &Path, registry: &str) -> Result<String> {
    if !path.exists() {
        return Err(anyhow::anyhow!("credentials file not found"));
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read credentials file: {}", path.display()))?;

    let credentials: toml::Value = toml::from_str(&content)
        .with_context(|| format!("failed to parse credentials file: {}", path.display()))?;

    // For crates-io, check [registry] table first, then [registries.crates-io]
    if registry == CRATES_IO_REGISTRY
        && let Some(token) = credentials
            .get("registry")
            .and_then(|r| r.get("token"))
            .and_then(|t| t.as_str())
    {
        return Ok(token.to_string());
    }

    // Check [registries.<name>] table
    if let Some(token) = credentials
        .get("registries")
        .and_then(|r| r.get(registry))
        .and_then(|r| r.get("token"))
        .and_then(|t| t.as_str())
    {
        return Ok(token.to_string());
    }

    // Check if it's a simple key-value format for default registry
    if registry == CRATES_IO_REGISTRY
        && let Some(token) = credentials.get("token").and_then(|t| t.as_str())
    {
        return Ok(token.to_string());
    }

    Err(anyhow::anyhow!(
        "token not found for registry: {}",
        registry
    ))
}

/// Parse credentials.toml and return all configured registries.
pub fn list_configured_registries(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read credentials file: {}", path.display()))?;

    let credentials: toml::Value = toml::from_str(&content)
        .with_context(|| format!("failed to parse credentials file: {}", path.display()))?;

    let mut registries = Vec::new();

    // Check for default registry
    if credentials.get("registry").is_some() || credentials.get("token").is_some() {
        registries.push(CRATES_IO_REGISTRY.to_string());
    }

    // Check for other registries
    if let Some(regs) = credentials.get("registries").and_then(|r| r.as_table()) {
        for name in regs.keys() {
            registries.push(name.clone());
        }
    }

    Ok(registries)
}

/// Mask a token for safe display (show first 4 and last 4 chars).
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
    fn cargo_home_path_uses_env() {
        let td = tempdir().expect("tempdir");
        let path = cargo_home_path(Some(td.path()));
        assert_eq!(path, td.path());
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
    fn token_from_credentials_file_crates_io() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);

        let content = r#"
[registry]
token = "creds-token"
"#;
        std::fs::write(&path, content).expect("write");

        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "creds-token");
    }

    #[test]
    fn token_from_credentials_file_custom_registry() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);

        let content = r#"
[registries.my-registry]
token = "custom-creds-token"
"#;
        std::fs::write(&path, content).expect("write");

        let token = token_from_credentials_file(&path, "my-registry").unwrap();
        assert_eq!(token, "custom-creds-token");
    }

    #[test]
    fn token_from_credentials_file_missing() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("nonexistent.toml");

        let result = token_from_credentials_file(&path, CRATES_IO_REGISTRY);
        assert!(result.is_err());
    }

    #[test]
    fn list_configured_registries_works() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);

        let content = r#"
[registry]
token = "default-token"

[registries.custom]
token = "custom-token"
"#;
        std::fs::write(&path, content).expect("write");

        let registries = list_configured_registries(&path).unwrap();
        assert!(registries.contains(&CRATES_IO_REGISTRY.to_string()));
        assert!(registries.contains(&"custom".to_string()));
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

    // --- Additional comprehensive tests ---

    #[test]
    fn auth_info_default_values() {
        let info = AuthInfo::default();
        assert!(info.token.is_none());
        assert_eq!(info.source, TokenSource::None);
        assert!(!info.detected);
    }

    #[test]
    fn mask_token_empty() {
        assert_eq!(mask_token(""), "");
    }

    #[test]
    fn mask_token_boundary_nine_chars() {
        // 9 chars is the first length > 8, so masking applies
        assert_eq!(mask_token("123456789"), "1234****6789");
    }

    #[test]
    fn mask_token_exactly_eight_chars() {
        // 8 chars => fully masked
        assert_eq!(mask_token("12345678"), "********");
    }

    #[test]
    fn mask_token_single_char() {
        assert_eq!(mask_token("x"), "*");
    }

    #[test]
    fn resolve_token_empty_env_is_skipped() {
        // An empty CARGO_REGISTRY_TOKEN should be treated as not set
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
        // An empty CARGO_REGISTRIES_MY_REG_TOKEN should be treated as not set
        temp_env::with_var("CARGO_REGISTRIES_MY_REG_TOKEN", Some(""), || {
            let td = tempdir().expect("tempdir");
            let auth = resolve_token("my-reg", Some(td.path()));
            assert!(!auth.detected);
            assert!(auth.token.is_none());
        });
    }

    #[test]
    fn resolve_token_empty_registry_name_uses_default_env() {
        // Empty registry name should check CARGO_REGISTRY_TOKEN (line 102)
        temp_env::with_var(CARGO_REGISTRY_TOKEN_ENV, Some("default-tok"), || {
            let auth = resolve_token("", None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("default-tok".to_string()));
            assert_eq!(auth.source, TokenSource::EnvDefault);
        });
    }

    #[test]
    fn resolve_token_custom_registry_ignores_default_env() {
        // A custom registry should NOT use CARGO_REGISTRY_TOKEN
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
    fn credentials_file_malformed_toml() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "this is not valid toml [[[").expect("write");

        let result = token_from_credentials_file(&path, CRATES_IO_REGISTRY);
        assert!(result.is_err());
    }

    #[test]
    fn credentials_file_simple_key_value_format() {
        // The legacy `token = "..."` at the top level for crates-io
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "token = \"legacy-token\"\n").expect("write");

        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "legacy-token");
    }

    #[test]
    fn credentials_file_registry_section_takes_priority_over_simple_key() {
        // [registry] table is checked before top-level `token`
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        let content = r#"
token = "legacy-token"

[registry]
token = "section-token"
"#;
        std::fs::write(&path, content).expect("write");

        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "section-token");
    }

    #[test]
    fn credentials_file_no_token_for_missing_registry() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "[registry]\ntoken = \"tok\"\n").expect("write");

        let result = token_from_credentials_file(&path, "nonexistent-registry");
        assert!(result.is_err());
    }

    #[test]
    fn credentials_file_empty_file() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "").expect("write");

        let result = token_from_credentials_file(&path, CRATES_IO_REGISTRY);
        assert!(result.is_err());
    }

    #[test]
    fn list_configured_registries_nonexistent_file() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("nonexistent.toml");
        let result = list_configured_registries(&path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_configured_registries_with_top_level_token() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "token = \"top-level\"\n").expect("write");

        let result = list_configured_registries(&path).unwrap();
        assert!(result.contains(&CRATES_IO_REGISTRY.to_string()));
    }

    #[test]
    fn list_configured_registries_malformed_file() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "not valid toml [[[").expect("write");

        let result = list_configured_registries(&path);
        assert!(result.is_err());
    }

    #[test]
    fn list_configured_registries_empty_file() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "").expect("write");

        let result = list_configured_registries(&path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_configured_registries_multiple_custom() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        let content = r#"
[registries.alpha]
token = "a"

[registries.beta]
token = "b"

[registries.gamma]
token = "c"
"#;
        std::fs::write(&path, content).expect("write");

        let result = list_configured_registries(&path).unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.contains(&"alpha".to_string()));
        assert!(result.contains(&"beta".to_string()));
        assert!(result.contains(&"gamma".to_string()));
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
        // "my-custom-reg" -> CARGO_REGISTRIES_MY_CUSTOM_REG_TOKEN
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
        // "myReg" -> CARGO_REGISTRIES_MYREG_TOKEN
        temp_env::with_var("CARGO_REGISTRIES_MYREG_TOKEN", Some("upper-tok"), || {
            let auth = resolve_token("myReg", None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("upper-tok".to_string()));
            assert_eq!(auth.source, TokenSource::EnvRegistry);
        });
    }

    // --- Snapshot tests (insta) ---

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
        fn snapshot_error_missing_credentials_file() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join("nonexistent.toml");
            let err = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap_err();
            assert_debug_snapshot!(err.to_string());
        }

        #[test]
        fn snapshot_error_malformed_credentials_file() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&path, "this is not valid toml [[[").expect("write");

            let err = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap_err();
            // Snapshot just the root cause message, not the path-dependent context
            let msg = err.root_cause().to_string();
            assert_debug_snapshot!(msg);
        }

        #[test]
        fn snapshot_error_token_not_found_for_registry() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&path, "[registry]\ntoken = \"tok\"\n").expect("write");

            let err = token_from_credentials_file(&path, "nonexistent-registry").unwrap_err();
            assert_debug_snapshot!(err.to_string());
        }

        #[test]
        fn snapshot_credentials_crates_io_registry_section() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            let content = r#"
[registry]
token = "crates-io-token"
"#;
            std::fs::write(&path, content).expect("write");
            let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
            assert_debug_snapshot!(token);
        }

        #[test]
        fn snapshot_credentials_custom_registry_section() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            let content = r#"
[registries.my-custom-registry]
token = "custom-reg-token"
"#;
            std::fs::write(&path, content).expect("write");
            let token = token_from_credentials_file(&path, "my-custom-registry").unwrap();
            assert_debug_snapshot!(token);
        }

        #[test]
        fn snapshot_credentials_legacy_toplevel_format() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&path, "token = \"legacy-format-token\"\n").expect("write");
            let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
            assert_debug_snapshot!(token);
        }

        #[test]
        fn snapshot_list_configured_registries_mixed() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            let content = r#"
[registry]
token = "default-token"

[registries.alpha]
token = "a"

[registries.beta]
token = "b"
"#;
            std::fs::write(&path, content).expect("write");
            let mut registries = list_configured_registries(&path).unwrap();
            registries.sort();
            assert_debug_snapshot!(registries);
        }

        #[test]
        fn snapshot_list_configured_registries_empty() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&path, "").expect("write");
            let registries = list_configured_registries(&path).unwrap();
            assert_debug_snapshot!(registries);
        }

        #[test]
        fn snapshot_auth_info_default() {
            let info = AuthInfo::default();
            assert_debug_snapshot!(info);
        }
    }

    // --- Property-based tests (proptest) ---

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for non-empty token strings (ASCII printable, no control chars).
        fn token_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9_\\-\\.]{1,128}"
        }

        /// Strategy for valid TOML-safe registry names (lowercase alphanumeric + hyphens).
        fn registry_name_strategy() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9\\-]{0,20}"
        }

        proptest! {
            /// Token strings written to a credentials file can be read back unchanged.
            #[test]
            fn token_roundtrip_via_credentials_file(token in token_strategy()) {
                let td = tempfile::tempdir().expect("tempdir");
                let path = td.path().join(CREDENTIALS_FILE);
                let content = format!("[registry]\ntoken = \"{token}\"\n");
                std::fs::write(&path, &content).expect("write");

                let result = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
                prop_assert_eq!(result, token);
            }

            /// Token strings written under [registries.<name>] can be read back unchanged.
            #[test]
            fn token_roundtrip_custom_registry(
                name in registry_name_strategy(),
                token in token_strategy(),
            ) {
                let td = tempfile::tempdir().expect("tempdir");
                let path = td.path().join(CREDENTIALS_FILE);
                let content = format!("[registries.{name}]\ntoken = \"{token}\"\n");
                std::fs::write(&path, &content).expect("write");

                let result = token_from_credentials_file(&path, &name).unwrap();
                prop_assert_eq!(result, token);
            }

            /// Credentials files with both [registry] and [registries.*] sections
            /// parse correctly: [registry] for crates-io, [registries.x] for custom.
            #[test]
            fn credentials_file_mixed_sections(
                default_token in token_strategy(),
                custom_name in registry_name_strategy(),
                custom_token in token_strategy(),
            ) {
                let td = tempfile::tempdir().expect("tempdir");
                let path = td.path().join(CREDENTIALS_FILE);
                let content = format!(
                    "[registry]\ntoken = \"{default_token}\"\n\n[registries.{custom_name}]\ntoken = \"{custom_token}\"\n"
                );
                std::fs::write(&path, &content).expect("write");

                let default = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
                prop_assert_eq!(default, default_token);

                let custom = token_from_credentials_file(&path, &custom_name).unwrap();
                prop_assert_eq!(custom, custom_token);
            }

            /// Environment variable token resolution works for arbitrary non-empty tokens
            /// via CARGO_REGISTRY_TOKEN for crates-io.
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

            /// Environment variable token resolution works for arbitrary registry names
            /// via CARGO_REGISTRIES_<NAME>_TOKEN.
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

            /// Token precedence: env var always wins over credentials file.
            /// For crates-io, CARGO_REGISTRY_TOKEN takes priority.
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

            /// Token precedence for custom registries: CARGO_REGISTRIES_<NAME>_TOKEN
            /// wins over [registries.<name>] in credentials file.
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

            /// mask_token always returns a string of the same or shorter length,
            /// and never exposes the middle of the token.
            #[test]
            fn mask_token_never_exposes_middle(token in "[[:ascii:]]{1,200}") {
                let masked = mask_token(&token);
                if token.len() <= 8 {
                    // Fully masked
                    prop_assert!(masked.chars().all(|c| c == '*'));
                    prop_assert_eq!(masked.len(), token.len());
                } else {
                    // Starts with first 4, ends with last 4, middle is "****"
                    prop_assert!(masked.starts_with(&token[..4]));
                    prop_assert!(masked.ends_with(&token[token.len() - 4..]));
                    prop_assert!(masked.contains("****"));
                }
            }
        }
    }
}
