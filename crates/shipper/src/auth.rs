use std::env;
use std::fs;
use std::path::PathBuf;

use crate::types::AuthType;
use anyhow::{Context, Result};

/// Attempt to resolve a registry token the way Cargo users typically configure it.
///
/// Resolution order:
/// 1) Environment variables (`CARGO_REGISTRY_TOKEN` for crates.io, or `CARGO_REGISTRIES_<NAME>_TOKEN`)
/// 2) `$CARGO_HOME/credentials.toml` (created by `cargo login`)
/// 3) `$CARGO_HOME/credentials` (legacy)
///
/// Returns `Ok(None)` if nothing is configured.
pub fn resolve_token(registry_name: &str) -> Result<Option<String>> {
    if let Some(tok) = token_from_env(registry_name) {
        return Ok(Some(tok));
    }

    let cargo_home = cargo_home_dir()?;
    for filename in ["credentials.toml", "credentials"] {
        let path = cargo_home.join(filename);
        if path.exists()
            && let Some(tok) = token_from_credentials_file(&path, registry_name)?
        {
            return Ok(Some(tok));
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

fn token_from_env(registry_name: &str) -> Option<String> {
    // The default registry (crates.io) has a special env var name.
    if registry_name == "crates-io"
        && let Ok(v) = env::var("CARGO_REGISTRY_TOKEN")
    {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }

    let env_name = format!(
        "CARGO_REGISTRIES_{}_TOKEN",
        normalize_registry_for_env(registry_name)
    );
    if let Ok(v) = env::var(env_name) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }

    None
}

fn cargo_home_dir() -> Result<PathBuf> {
    if let Ok(ch) = env::var("CARGO_HOME") {
        return Ok(PathBuf::from(ch));
    }

    let home = env::var("HOME").context("HOME env var not set; set CARGO_HOME or HOME")?;
    Ok(PathBuf::from(home).join(".cargo"))
}

fn token_from_credentials_file(path: &PathBuf, registry_name: &str) -> Result<Option<String>> {
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
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Ok(Some(tok));
        }
    }

    // Other registries (and sometimes crates.io) can use `[registries.<name>] token = "..."`.
    if let Some(tok) = value
        .get("registries")
        .and_then(|t| t.get(registry_name))
        .and_then(|t| t.get("token"))
        .and_then(|v| v.as_str())
    {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Ok(Some(tok));
        }
    }

    // Best-effort: try `crates-io` vs `crates.io` variants.
    if registry_name == "crates-io" {
        for alt in ["crates.io", "crates_io", "crates-io"] {
            if let Some(tok) = value
                .get("registries")
                .and_then(|t| t.get(alt))
                .and_then(|t| t.get("token"))
                .and_then(|v| v.as_str())
            {
                let tok = tok.trim().to_string();
                if !tok.is_empty() {
                    return Ok(Some(tok));
                }
            }
        }
    }

    Ok(None)
}

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
    use std::path::PathBuf;

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
    fn token_from_env_prefers_crates_io_default_var() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", Some("token-a")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("token-b")),
            ],
            || {
                let tok = token_from_env("crates-io");
                assert_eq!(tok.as_deref(), Some("token-a"));
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_uses_registry_specific_var() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_PRIVATE_REG_TOKEN", Some("abc123")),
            ],
            || {
                let tok = token_from_env("private-reg");
                assert_eq!(tok.as_deref(), Some("abc123"));
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_reads_crates_io_var_when_only_default_is_set() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", Some("solo-token")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let tok = token_from_env("crates-io");
                assert_eq!(tok.as_deref(), Some("solo-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_reads_named_registry_var_when_non_empty() {
        temp_env::with_var(
            "CARGO_REGISTRIES_ALT_REG_TOKEN",
            Some("named-token"),
            || {
                let tok = token_from_env("alt-reg");
                assert_eq!(tok.as_deref(), Some("named-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_ignores_empty_values() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", Some("   ")),
                ("CARGO_REGISTRIES_ALT_REG_TOKEN", Some(" ")),
            ],
            || {
                let crates_io = token_from_env("crates-io");
                let alt = token_from_env("alt-reg");
                assert!(crates_io.is_none());
                assert!(alt.is_none());
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_home_dir_prefers_cargo_home_env() {
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some("X:\\cargo-home")),
                ("HOME", Some("X:\\home")),
            ],
            || {
                let p = cargo_home_dir().expect("cargo home");
                assert_eq!(p, PathBuf::from("X:\\cargo-home"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_home_dir_falls_back_to_home() {
        temp_env::with_vars(
            [("CARGO_HOME", None::<&str>), ("HOME", Some("X:\\home"))],
            || {
                let p = cargo_home_dir().expect("cargo home");
                assert_eq!(p, PathBuf::from("X:\\home").join(".cargo"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_home_dir_errors_without_envs() {
        temp_env::with_vars(
            [("CARGO_HOME", None::<&str>), ("HOME", None::<&str>)],
            || {
                let err = cargo_home_dir().expect_err("must fail");
                assert!(format!("{err:#}").contains("HOME env var not set"));
            },
        );
    }

    #[test]
    fn token_from_credentials_file_parses_registry_table() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = "  secret  "
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("secret"));
    }

    #[test]
    fn token_from_credentials_file_parses_named_registry() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registries.private_reg]
token = "token-x"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "private_reg").expect("parse");
        assert_eq!(tok.as_deref(), Some("token-x"));
    }

    #[test]
    fn token_from_credentials_file_supports_crates_io_aliases() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registries.crates-io]
token = "token-dot"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("token-dot"));
    }

    #[test]
    fn token_from_credentials_file_reports_parse_error() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "[broken").expect("write");

        let err = token_from_credentials_file(&path, "crates-io").expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to parse cargo credentials file as TOML"));
    }

    #[test]
    fn token_from_credentials_file_reports_read_error() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("missing.toml");
        let err = token_from_credentials_file(&path, "crates-io").expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to read cargo credentials file"));
    }

    #[test]
    fn token_from_credentials_file_returns_none_when_missing_tokens() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = ""
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(tok.is_none());
    }

    #[test]
    fn token_from_credentials_file_ignores_empty_named_and_alias_tokens() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registries.private_reg]
token = "   "

[registries.crates-io]
token = "  "
"#,
        )
        .expect("write");

        let named = token_from_credentials_file(&path, "private_reg").expect("parse");
        assert!(named.is_none());

        let crates = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(crates.is_none());
    }

    #[test]
    #[serial]
    fn resolve_token_prefers_env_then_credentials() {
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
    fn resolve_token_reads_credentials_when_env_missing() {
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
                ("CARGO_REGISTRIES_PRIVATE_REG_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("private-reg").expect("resolve");
                assert_eq!(tok.as_deref(), Some("legacy-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_reads_credentials_toml_before_legacy_file() {
        let td = tempdir().expect("tempdir");

        fs::write(
            td.path().join("credentials.toml"),
            r#"[registry]
token = "toml-token"
"#,
        )
        .expect("write");
        fs::write(
            td.path().join("credentials"),
            r#"[registry]
token = "legacy-token"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("crates-io").expect("resolve");
                assert_eq!(tok.as_deref(), Some("toml-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_skips_empty_primary_credentials_and_uses_legacy() {
        let td = tempdir().expect("tempdir");

        fs::write(
            td.path().join("credentials.toml"),
            r#"[registry]
token = " "
"#,
        )
        .expect("write");
        fs::write(
            td.path().join("credentials"),
            r#"[registry]
token = "fallback-token"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("crates-io").expect("resolve");
                assert_eq!(tok.as_deref(), Some("fallback-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_returns_none_when_unconfigured() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_PRIVATE_REG_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("private-reg").expect("resolve");
                assert!(tok.is_none());
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
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("oidc-token")),
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
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    Some("https://example.invalid/oidc"),
                ),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
            ],
            || {
                let auth = detect_auth_type("crates-io").expect("detect");
                assert_eq!(auth, Some(AuthType::Unknown));
            },
        );
    }

    #[test]
    #[serial]
    fn detect_auth_type_returns_none_without_known_auth() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", None::<&str>),
            ],
            || {
                let auth = detect_auth_type("crates-io").expect("detect");
                assert!(auth.is_none());
            },
        );
    }

    // --- normalize_registry_for_env edge cases ---

    #[test]
    fn normalize_registry_handles_dots_and_hyphens() {
        assert_eq!(normalize_registry_for_env("my.fancy-reg"), "MY_FANCY_REG");
    }

    #[test]
    fn normalize_registry_handles_all_special_chars() {
        assert_eq!(normalize_registry_for_env("a!@#b"), "A___B");
    }

    #[test]
    fn normalize_registry_handles_empty_string() {
        assert_eq!(normalize_registry_for_env(""), "");
    }

    #[test]
    fn normalize_registry_preserves_digits() {
        assert_eq!(normalize_registry_for_env("reg123-v2"), "REG123_V2");
    }

    // --- token_from_env edge cases ---

    #[test]
    #[serial]
    fn token_from_env_returns_none_when_no_vars_set() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                assert!(token_from_env("crates-io").is_none());
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_returns_none_for_unknown_registry() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_NONEXISTENT_TOKEN", None::<&str>),
            ],
            || {
                assert!(token_from_env("nonexistent").is_none());
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_trims_whitespace_from_value() {
        temp_env::with_var("CARGO_REGISTRY_TOKEN", Some("  trimmed-tok  "), || {
            let tok = token_from_env("crates-io");
            assert_eq!(tok.as_deref(), Some("trimmed-tok"));
        });
    }

    #[test]
    #[serial]
    fn token_from_env_ignores_empty_default_but_uses_named() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", Some("")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("named-fallback")),
            ],
            || {
                let tok = token_from_env("crates-io");
                assert_eq!(tok.as_deref(), Some("named-fallback"));
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_with_special_chars_in_token() {
        let special_token = "cio_abc+/=@#$%^&*()_🦀";
        temp_env::with_var("CARGO_REGISTRY_TOKEN", Some(special_token), || {
            let tok = token_from_env("crates-io");
            assert_eq!(tok.as_deref(), Some(special_token));
        });
    }

    #[test]
    #[serial]
    fn token_from_env_with_base64_encoded_token() {
        let b64_token = "Y2lvX2FiY0RFRjEyMzQ1Njc4OQ==";
        temp_env::with_var("CARGO_REGISTRY_TOKEN", Some(b64_token), || {
            let tok = token_from_env("crates-io");
            assert_eq!(tok.as_deref(), Some(b64_token));
        });
    }

    #[test]
    #[serial]
    fn token_from_env_non_default_registry_ignores_cargo_registry_token() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", Some("default-only")),
                ("CARGO_REGISTRIES_MY_PRIVATE_TOKEN", None::<&str>),
            ],
            || {
                assert!(token_from_env("my-private").is_none());
            },
        );
    }

    #[test]
    #[serial]
    fn token_from_env_crates_io_falls_through_to_named_var() {
        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("via-named")),
            ],
            || {
                let tok = token_from_env("crates-io");
                assert_eq!(tok.as_deref(), Some("via-named"));
            },
        );
    }

    // --- credentials.toml parsing edge cases ---

    #[test]
    fn token_from_credentials_file_empty_file() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "").expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(tok.is_none());
    }

    #[test]
    fn token_from_credentials_file_no_token_key_in_registry() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
other_key = "some_value"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(tok.is_none());
    }

    #[test]
    fn token_from_credentials_file_token_is_integer_not_string() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = 12345
"#,
        )
        .expect("write");

        // token is not a string, so as_str() returns None
        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(tok.is_none());
    }

    #[test]
    fn token_from_credentials_file_token_is_boolean() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = true
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(tok.is_none());
    }

    #[test]
    fn token_from_credentials_file_multiple_registries() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = "default-token"

[registries.alpha]
token = "alpha-token"

[registries.beta]
token = "beta-token"
"#,
        )
        .expect("write");

        let default = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(default.as_deref(), Some("default-token"));

        let alpha = token_from_credentials_file(&path, "alpha").expect("parse");
        assert_eq!(alpha.as_deref(), Some("alpha-token"));

        let beta = token_from_credentials_file(&path, "beta").expect("parse");
        assert_eq!(beta.as_deref(), Some("beta-token"));

        let missing = token_from_credentials_file(&path, "gamma").expect("parse");
        assert!(missing.is_none());
    }

    #[test]
    fn token_from_credentials_file_crates_io_registry_table_takes_precedence_over_registries() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = "primary-token"

[registries.crates-io]
token = "secondary-token"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("primary-token"));
    }

    #[test]
    fn token_from_credentials_file_crates_io_dot_alias() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registries."crates.io"]
token = "dotted-token"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("dotted-token"));
    }

    #[test]
    fn token_from_credentials_file_crates_io_underscore_alias() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registries.crates_io]
token = "underscore-token"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("underscore-token"));
    }

    #[test]
    fn token_from_credentials_file_special_chars_in_token_value() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "[registry]\ntoken = \"cio_abc+/=🦀@special\"\n").expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("cio_abc+/=🦀@special"));
    }

    #[test]
    fn token_from_credentials_file_base64_token_value() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            "[registry]\ntoken = \"dGhpcyBpcyBhIGJhc2U2NCB0b2tlbg==\"\n",
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("dGhpcyBpcyBhIGJhc2U2NCB0b2tlbg=="));
    }

    #[test]
    fn token_from_credentials_file_whitespace_only_token_returns_none() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "[registry]\ntoken = \"   \\t  \"\n").expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(tok.is_none());
    }

    #[test]
    fn token_from_credentials_file_named_registry_not_found() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = "default"

[registries.alpha]
token = "alpha"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "beta").expect("parse");
        assert!(tok.is_none());
    }

    #[test]
    fn token_from_credentials_file_extra_keys_ignored() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(
            &path,
            r#"[registry]
token = "my-token"
secret = "ignored"
username = "also-ignored"

[other_section]
key = "value"
"#,
        )
        .expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert_eq!(tok.as_deref(), Some("my-token"));
    }

    #[test]
    fn token_from_credentials_file_malformed_toml_missing_value() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "[registry]\ntoken = \n").expect("write");

        let err = token_from_credentials_file(&path, "crates-io").expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to parse cargo credentials file as TOML"));
    }

    #[test]
    fn token_from_credentials_file_malformed_toml_unclosed_string() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "[registry]\ntoken = \"unclosed\n").expect("write");

        let err = token_from_credentials_file(&path, "crates-io").expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to parse cargo credentials file as TOML"));
    }

    #[test]
    fn token_from_credentials_file_malformed_toml_duplicate_keys() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "[registry]\ntoken = \"first\"\ntoken = \"second\"\n").expect("write");

        let err = token_from_credentials_file(&path, "crates-io").expect_err("must fail");
        assert!(format!("{err:#}").contains("failed to parse cargo credentials file as TOML"));
    }

    #[test]
    fn token_from_credentials_file_only_comments() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join("credentials.toml");
        fs::write(&path, "# just a comment\n# another one\n").expect("write");

        let tok = token_from_credentials_file(&path, "crates-io").expect("parse");
        assert!(tok.is_none());
    }

    // --- resolve_token integration edge cases ---

    #[test]
    #[serial]
    fn resolve_token_env_overrides_credentials_for_named_registry() {
        let td = tempdir().expect("tempdir");
        fs::write(
            td.path().join("credentials.toml"),
            r#"[registries.my-reg]
token = "file-token"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRIES_MY_REG_TOKEN", Some("env-token")),
            ],
            || {
                let tok = resolve_token("my-reg").expect("resolve");
                assert_eq!(tok.as_deref(), Some("env-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_falls_back_to_credentials_for_named_registry() {
        let td = tempdir().expect("tempdir");
        fs::write(
            td.path().join("credentials.toml"),
            r#"[registries.my-reg]
token = "file-token"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRIES_MY_REG_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("my-reg").expect("resolve");
                assert_eq!(tok.as_deref(), Some("file-token"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_returns_none_for_empty_env_and_empty_file_token() {
        let td = tempdir().expect("tempdir");
        fs::write(
            td.path().join("credentials.toml"),
            r#"[registry]
token = "   "
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", Some("")),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", Some("  ")),
            ],
            || {
                let tok = resolve_token("crates-io").expect("resolve");
                // credentials.toml also has whitespace-only, and no legacy file
                assert!(tok.is_none());
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_uses_legacy_credentials_file_for_named_registry() {
        let td = tempdir().expect("tempdir");
        // No credentials.toml, only legacy `credentials`
        fs::write(
            td.path().join("credentials"),
            r#"[registries.custom]
token = "legacy-custom"
"#,
        )
        .expect("write");

        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRIES_CUSTOM_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("custom").expect("resolve");
                assert_eq!(tok.as_deref(), Some("legacy-custom"));
            },
        );
    }

    #[test]
    #[serial]
    fn resolve_token_returns_none_with_no_files_and_no_env() {
        let td = tempdir().expect("tempdir");
        // Empty CARGO_HOME, no credentials files at all
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
            ],
            || {
                let tok = resolve_token("crates-io").expect("resolve");
                assert!(tok.is_none());
            },
        );
    }

    // --- detect_auth_type_from_token unit tests ---

    #[test]
    fn detect_auth_type_from_token_returns_token_for_nonempty() {
        assert_eq!(
            detect_auth_type_from_token(Some("abc123")),
            Some(AuthType::Token)
        );
    }

    #[test]
    fn detect_auth_type_from_token_returns_none_for_none() {
        // Without OIDC env vars, this would return None in isolation.
        // detect_auth_type_from_token only checks OIDC env directly, so we test
        // the token-absent case in serial tests that control env.
        let result = detect_auth_type_from_token(None);
        // Result depends on ambient OIDC env vars; just verify it's not Token
        assert_ne!(result, Some(AuthType::Token));
    }

    #[test]
    fn detect_auth_type_from_token_returns_none_for_empty_string() {
        let result = detect_auth_type_from_token(Some(""));
        assert_ne!(result, Some(AuthType::Token));
    }

    #[test]
    fn detect_auth_type_from_token_returns_none_for_whitespace_only() {
        let result = detect_auth_type_from_token(Some("   \t  "));
        assert_ne!(result, Some(AuthType::Token));
    }

    #[test]
    fn detect_auth_type_from_token_accepts_special_chars() {
        assert_eq!(
            detect_auth_type_from_token(Some("cio_🦀+/=")),
            Some(AuthType::Token)
        );
    }

    // --- detect_auth_type with partial OIDC (only token, no URL) ---

    #[test]
    #[serial]
    fn detect_auth_type_returns_unknown_for_only_oidc_token() {
        let td = tempdir().expect("tempdir");
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some(td.path().to_str().expect("utf8"))),
                ("CARGO_REGISTRY_TOKEN", None::<&str>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_URL", None::<&str>),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", Some("just-token")),
            ],
            || {
                let auth = detect_auth_type("crates-io").expect("detect");
                assert_eq!(auth, Some(AuthType::Unknown));
            },
        );
    }

    // --- cargo_home_dir edge cases ---

    #[test]
    #[serial]
    fn cargo_home_dir_uses_cargo_home_even_if_empty_home() {
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some("C:\\custom\\cargo")),
                ("HOME", None::<&str>),
            ],
            || {
                let p = cargo_home_dir().expect("cargo home");
                assert_eq!(p, PathBuf::from("C:\\custom\\cargo"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_home_dir_with_unicode_path() {
        temp_env::with_vars(
            [
                ("CARGO_HOME", Some("C:\\用户\\cargo")),
                ("HOME", None::<&str>),
            ],
            || {
                let p = cargo_home_dir().expect("cargo home");
                assert_eq!(p, PathBuf::from("C:\\用户\\cargo"));
            },
        );
    }
}
