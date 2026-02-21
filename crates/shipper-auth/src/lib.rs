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

    Err(anyhow::anyhow!("token not found for registry: {}", registry))
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
    format!(
        "{}****{}",
        &token[..4],
        &token[token.len() - 4..]
    )
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
        temp_env::with_var("CARGO_REGISTRIES_MY_REGISTRY_TOKEN", Some("custom-token"), || {
            let auth = resolve_token("my-registry", None);
            assert!(auth.detected);
            assert_eq!(auth.token, Some("custom-token".to_string()));
            assert_eq!(auth.source, TokenSource::EnvRegistry);
        });
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
        assert_eq!(TokenSource::EnvRegistry.to_string(), "CARGO_REGISTRIES_<NAME>_TOKEN");
        assert_eq!(TokenSource::CredentialsFile.to_string(), "credentials.toml");
    }
}