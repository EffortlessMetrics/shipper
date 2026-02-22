use std::env;
use std::fs;
use std::path::PathBuf;

use crate::types::AuthType;
use anyhow::{Context, Result};

// Intentionally keep the public surface aligned with the in-crate `auth` module.
// We delegate to the microcrate for most behavior, while preserving legacy
// normalization and fallback behavior from the monolithic module.
pub fn resolve_token(registry_name: &str) -> Result<Option<String>> {
    let micro_token = shipper_auth::resolve_token(registry_name, None)
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
    for filename in [shipper_auth::CREDENTIALS_FILE, "credentials"] {
        let path = cargo_home.join(filename);
        if path.exists()
            && let Some(token) = token_from_credentials_file(&path, registry_name)?
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
/// 1) Explicit Cargo token configuration (`AuthType::Token`)
/// 2) Trusted publishing OIDC environment (`AuthType::TrustedPublishing`)
/// 3) Partial trusted-publishing environment (`AuthType::Unknown`)
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

fn token_from_credentials_file(
    path: &std::path::Path,
    registry_name: &str,
) -> Result<Option<String>> {
    let content = fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read cargo credentials file at {}",
            path.display()
        )
    })?;

    let value: toml::Value = toml::from_str(&content).with_context(|| {
        format!(
            "failed to parse cargo credentials file as TOML: {}",
            path.display()
        )
    })?;

    // crates.io commonly uses `[registry] token = "..."`.
    if registry_name == "crates-io"
        && let Some(tok) = value
            .get("registry")
            .and_then(|t| t.get("token"))
            .and_then(|v| v.as_str())
    {
        return Ok(Some(tok.to_string()));
    }

    // Other registries (and sometimes crates.io) can use `[registries.<name>] token = "..."`.
    if let Some(tok) = value
        .get("registries")
        .and_then(|t| t.get(registry_name))
        .and_then(|t| t.get("token"))
        .and_then(|v| v.as_str())
    {
        return Ok(Some(tok.to_string()));
    }

    // Best-effort: try `crates-io` vs `crates.io` vs `crates_io` variants.
    if registry_name == "crates-io" {
        for alt in ["crates.io", "crates_io"] {
            if let Some(tok) = value
                .get("registries")
                .and_then(|t| t.get(alt))
                .and_then(|t| t.get("token"))
                .and_then(|v| v.as_str())
            {
                return Ok(Some(tok.to_string()));
            }
        }

        // Unquoted `[registries.crates.io]` in TOML creates nested tables
        // (registries -> crates -> io) rather than a single dotted key.
        if let Some(tok) = value
            .get("registries")
            .and_then(|t| t.get("crates"))
            .and_then(|t| t.get("io"))
            .and_then(|t| t.get("token"))
            .and_then(|v| v.as_str())
        {
            return Ok(Some(tok.to_string()));
        }
    }

    Ok(None)
}

fn cargo_home_dir() -> Result<PathBuf> {
    if let Ok(ch) = env::var("CARGO_HOME") {
        return Ok(PathBuf::from(ch));
    }

    let home = env::var("HOME").context("HOME env var not set; set CARGO_HOME or HOME")?;
    Ok(PathBuf::from(home).join(".cargo"))
}

#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

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
