//! Cargo registry token resolution and authentication detection.
//!
//! This module is the single source of truth for Shipper's authentication
//! handling. It was previously split across:
//!   - a `shipper-auth` microcrate (token resolution from env/credentials),
//!   - an in-crate `auth` shim (added whitespace trimming, legacy credentials
//!     filename fallback, and OIDC-based `detect_auth_type`).
//!
//! Both were absorbed into `crate::ops::auth` and split into focused
//! submodules:
//!
//! - [`credentials`] — `$CARGO_HOME/credentials.toml` (and legacy `credentials`)
//!   file parsing, plus crates.io alias handling.
//! - [`resolver`] — token discovery across env vars and credentials files,
//!   along with the `AuthInfo`/`TokenSource` diagnostic types.
//! - [`oidc`] — GitHub Actions trusted-publishing detection.
//!
//! # Resolution order
//!
//! 1. `CARGO_REGISTRY_TOKEN` env var (only for `crates-io` / empty name)
//! 2. `CARGO_REGISTRIES_<NAME>_TOKEN` env var
//! 3. `$CARGO_HOME/credentials.toml` (with crates.io aliases: `crates-io`,
//!    `crates.io`, `crates_io`, and the nested `[registries.crates.io]` form)
//! 4. Legacy `$CARGO_HOME/credentials` file
//!
//! # Invariants
//!
//! - Tokens are opaque strings; NEVER log them.
//! - Empty and whitespace-trimmed-empty tokens are treated as absent.
//! - OIDC detection requires BOTH `ACTIONS_ID_TOKEN_REQUEST_URL` and
//!   `ACTIONS_ID_TOKEN_REQUEST_TOKEN`.

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::types::AuthType;

pub(crate) mod credentials;
pub(crate) mod oidc;
pub(crate) mod resolver;

// Re-exports forming the public-to-crate API. Exposed further as
// `shipper::auth::*` by the facade module in `lib.rs`.
pub use credentials::{CREDENTIALS_FILE, list_configured_registries};
pub use oidc::is_trusted_publishing_available;
pub use resolver::{
    AuthInfo, CARGO_HOME_ENV, CARGO_REGISTRIES_TOKEN_PREFIX, CARGO_REGISTRY_TOKEN_ENV,
    CRATES_IO_REGISTRY, TokenSource, cargo_home_path, has_token, mask_token,
    resolve_token as resolve_auth_info,
};

/// Resolve the authentication token for a registry.
///
/// Wraps the lower-level resolver (which returns an [`AuthInfo`] diagnostic
/// record) and adds:
///
/// - Whitespace trimming; empty/whitespace-only tokens are treated as absent.
/// - Fallback to the legacy `$CARGO_HOME/credentials` filename (in addition
///   to `credentials.toml`) with crates.io alias handling.
///
/// This is the canonical public-crate API used by `engine.rs` and the CLI.
pub fn resolve_token(registry_name: &str) -> Result<Option<String>> {
    let micro_token = resolver::resolve_token(registry_name, None)
        .token
        .and_then(|token| {
            let trimmed = token.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

    if micro_token.is_some() {
        return Ok(micro_token);
    }

    let cargo_home = cargo_home_dir()?;
    for filename in [CREDENTIALS_FILE, "credentials"] {
        let path = cargo_home.join(filename);
        if path.exists()
            && let Some(token) =
                credentials::token_from_credentials_file_extended(&path, registry_name)?
        {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Ok(Some(token));
            }
        }
    }

    Ok(None)
}

/// Detect the best-known authentication mode for publish/preflight diagnostics.
///
/// Resolution order:
/// 1) Explicit Cargo token configuration ([`AuthType::Token`])
/// 2) Trusted publishing OIDC environment ([`AuthType::TrustedPublishing`])
/// 3) Partial trusted-publishing environment ([`AuthType::Unknown`])
/// 4) No known auth configured (`None`)
pub fn detect_auth_type(registry_name: &str) -> Result<Option<AuthType>> {
    let token = resolve_token(registry_name)?;
    Ok(detect_auth_type_from_token(token.as_deref()))
}

pub(crate) fn detect_auth_type_from_token(token: Option<&str>) -> Option<AuthType> {
    if token.map(str::trim).map(|s| !s.is_empty()).unwrap_or(false) {
        return Some(AuthType::Token);
    }

    let has_oidc_url = env::var_os("ACTIONS_ID_TOKEN_REQUEST_URL").is_some();
    let has_oidc_token = env::var_os("ACTIONS_ID_TOKEN_REQUEST_TOKEN").is_some();

    match (has_oidc_url, has_oidc_token) {
        (true, true) => Some(AuthType::TrustedPublishing),
        (true, false) | (false, true) => Some(AuthType::Unknown),
        (false, false) => None,
    }
}

/// Resolve the `CARGO_HOME` directory via env-only lookup (uses `HOME` as
/// fallback). Distinct from [`resolver::cargo_home_path`] which also falls
/// back to `dirs::home_dir()` and the current directory.
///
/// The env-only form preserves the original shim behavior that expects a
/// clean error when neither `CARGO_HOME` nor `HOME` is set.
fn cargo_home_dir() -> Result<PathBuf> {
    if let Ok(ch) = env::var("CARGO_HOME") {
        return Ok(PathBuf::from(ch));
    }

    let home = env::var("HOME").context("HOME env var not set; set CARGO_HOME or HOME")?;
    Ok(PathBuf::from(home).join(".cargo"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use serial_test::serial;
    use tempfile::tempdir;

    fn normalize_registry_for_env(name: &str) -> String {
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect()
    }

    #[test]
    fn normalize_registry_name_for_env() {
        assert_eq!(normalize_registry_for_env("my-registry"), "MY_REGISTRY");
        assert_eq!(normalize_registry_for_env("crates.io"), "CRATES_IO");
        assert_eq!(normalize_registry_for_env("A1_b"), "A1_B");
    }

    #[test]
    #[serial]
    fn resolve_token_prefers_crates_io_default_var() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", Some("token-a")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("token-b")),
            ],
            || {
                let tok = resolve_token("crates-io").expect("resolve");
                assert_eq!(tok.as_deref(), Some("token-a"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_uses_env_registry_var() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_PRIVATE_REG_TOKEN", Some("abc123")),
            ],
            || {
                let tok = resolve_token("private-reg").expect("resolve");
                assert_eq!(tok.as_deref(), Some("abc123"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_prefers_env_over_credentials() {
        let td = tempdir().expect("tempdir");
        fs::write(
            td.path().join("credentials.toml"),
            r#"[registry]
token = "file-token"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", Some("env-token")),
            ],
            || {
                let tok = resolve_token("crates-io").expect("resolve");
                assert_eq!(tok.as_deref(), Some("env-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_reads_legacy_credentials_file() {
        let td = tempdir().expect("tempdir");
        fs::write(
            td.path().join("credentials"),
            r#"[registries.private-reg]
token = "legacy-token"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("private-reg").expect("resolve");
                assert_eq!(tok.as_deref(), Some("legacy-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_supports_crates_io_aliases_in_credentials() {
        let td = tempdir().expect("tempdir");
        fs::write(
            td.path().join("credentials.toml"),
            r#"[registries.crates.io]
token = "token-dot"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [("CARGO_HOME", Some(td.path().to_str().expect("utf8")))],
            || {
                let tok = resolve_token("crates-io").expect("resolve");
                assert_eq!(tok.as_deref(), Some("token-dot"));
            },
        );
    }

    #[test]
    #[serial]
    fn detect_auth_type_prefers_token_when_present() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", Some("env-token")),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("oidc-token")),
            ],
            || {
                let auth = detect_auth_type("crates-io").expect("detect");
                assert_eq!(auth, Some(AuthType::Token));
            },
        );
    }

    #[test]
    #[serial]
    fn detect_auth_type_detects_trusted_publishing_from_oidc_env() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("oidc-token")),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ],
            || {
                let auth = detect_auth_type("crates-io").expect("detect");
                assert_eq!(auth, Some(AuthType::TrustedPublishing));
            },
        );
    }

    #[test]
    #[serial]
    fn detect_auth_type_returns_unknown_for_partial_oidc_env() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ],
            || {
                let auth = detect_auth_type("crates-io").expect("detect");
                assert_eq!(auth, Some(AuthType::Unknown));
            },
        );
    }
}
