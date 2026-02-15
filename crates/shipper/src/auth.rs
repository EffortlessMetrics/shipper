use std::env;
use std::fs;
use std::path::PathBuf;

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

    struct EnvGuard {
        key: String,
        old: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let old = env::var(key).ok();
            unsafe { env::set_var(key, value) };
            Self {
                key: key.to_string(),
                old,
            }
        }

        fn unset(key: &str) -> Self {
            let old = env::var(key).ok();
            unsafe { env::remove_var(key) };
            Self {
                key: key.to_string(),
                old,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = &self.old {
                unsafe { env::set_var(&self.key, v) };
            } else {
                unsafe { env::remove_var(&self.key) };
            }
        }
    }

    #[test]
    fn normalize_registry_name_for_env() {
        assert_eq!(normalize_registry_for_env("my-registry"), "MY_REGISTRY");
        assert_eq!(normalize_registry_for_env("crates.io"), "CRATES_IO");
        assert_eq!(normalize_registry_for_env("A1_b"), "A1_B");
    }

    #[test]
    #[serial]
    fn token_from_env_prefers_crates_io_default_var() {
        let _a = EnvGuard::set("CARGO_REGISTRY_TOKEN", "token-a");
        let _b = EnvGuard::set("CARGO_REGISTRIES_CRATES_IO_TOKEN", "token-b");
        let tok = token_from_env("crates-io");
        assert_eq!(tok.as_deref(), Some("token-a"));
    }

    #[test]
    #[serial]
    fn token_from_env_uses_registry_specific_var() {
        let _a = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _b = EnvGuard::set("CARGO_REGISTRIES_PRIVATE_REG_TOKEN", "abc123");
        let tok = token_from_env("private-reg");
        assert_eq!(tok.as_deref(), Some("abc123"));
    }

    #[test]
    #[serial]
    fn token_from_env_reads_crates_io_var_when_only_default_is_set() {
        let _a = EnvGuard::set("CARGO_REGISTRY_TOKEN", "solo-token");
        let _b = EnvGuard::unset("CARGO_REGISTRIES_CRATES_IO_TOKEN");
        let tok = token_from_env("crates-io");
        assert_eq!(tok.as_deref(), Some("solo-token"));
    }

    #[test]
    #[serial]
    fn token_from_env_reads_named_registry_var_when_non_empty() {
        let _a = EnvGuard::set("CARGO_REGISTRIES_ALT_REG_TOKEN", "named-token");
        let tok = token_from_env("alt-reg");
        assert_eq!(tok.as_deref(), Some("named-token"));
    }

    #[test]
    #[serial]
    fn token_from_env_ignores_empty_values() {
        let _a = EnvGuard::set("CARGO_REGISTRY_TOKEN", "   ");
        let _b = EnvGuard::set("CARGO_REGISTRIES_ALT_REG_TOKEN", " ");
        let crates_io = token_from_env("crates-io");
        let alt = token_from_env("alt-reg");
        assert!(crates_io.is_none());
        assert!(alt.is_none());
    }

    #[test]
    #[serial]
    fn cargo_home_dir_prefers_cargo_home_env() {
        let _a = EnvGuard::set("CARGO_HOME", "X:\\cargo-home");
        let _b = EnvGuard::set("HOME", "X:\\home");
        let p = cargo_home_dir().expect("cargo home");
        assert_eq!(p, PathBuf::from("X:\\cargo-home"));
    }

    #[test]
    #[serial]
    fn cargo_home_dir_falls_back_to_home() {
        let _a = EnvGuard::unset("CARGO_HOME");
        let _b = EnvGuard::set("HOME", "X:\\home");
        let p = cargo_home_dir().expect("cargo home");
        assert_eq!(p, PathBuf::from("X:\\home").join(".cargo"));
    }

    #[test]
    #[serial]
    fn cargo_home_dir_errors_without_envs() {
        let _a = EnvGuard::unset("CARGO_HOME");
        let _b = EnvGuard::unset("HOME");
        let err = cargo_home_dir().expect_err("must fail");
        assert!(format!("{err:#}").contains("HOME env var not set"));
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
        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::set("CARGO_REGISTRY_TOKEN", "env-token");

        fs::write(
            td.path().join("credentials.toml"),
            r#"[registry]
token = "file-token"
"#,
        )
        .expect("write");

        let tok = resolve_token("crates-io").expect("resolve");
        assert_eq!(tok.as_deref(), Some("env-token"));
    }

    #[test]
    #[serial]
    fn resolve_token_reads_credentials_when_env_missing() {
        let td = tempdir().expect("tempdir");
        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _c = EnvGuard::unset("CARGO_REGISTRIES_PRIVATE_REG_TOKEN");
        fs::write(
            td.path().join("credentials"),
            r#"[registries.private-reg]
token = "legacy-token"
"#,
        )
        .expect("write");

        let tok = resolve_token("private-reg").expect("resolve");
        assert_eq!(tok.as_deref(), Some("legacy-token"));
    }

    #[test]
    #[serial]
    fn resolve_token_reads_credentials_toml_before_legacy_file() {
        let td = tempdir().expect("tempdir");
        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _c = EnvGuard::unset("CARGO_REGISTRIES_CRATES_IO_TOKEN");

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

        let tok = resolve_token("crates-io").expect("resolve");
        assert_eq!(tok.as_deref(), Some("toml-token"));
    }

    #[test]
    #[serial]
    fn resolve_token_skips_empty_primary_credentials_and_uses_legacy() {
        let td = tempdir().expect("tempdir");
        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _c = EnvGuard::unset("CARGO_REGISTRIES_CRATES_IO_TOKEN");

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

        let tok = resolve_token("crates-io").expect("resolve");
        assert_eq!(tok.as_deref(), Some("fallback-token"));
    }

    #[test]
    #[serial]
    fn resolve_token_returns_none_when_unconfigured() {
        let td = tempdir().expect("tempdir");
        let _a = EnvGuard::set("CARGO_HOME", td.path().to_str().expect("utf8"));
        let _b = EnvGuard::unset("CARGO_REGISTRY_TOKEN");
        let _c = EnvGuard::unset("CARGO_REGISTRIES_PRIVATE_REG_TOKEN");
        let tok = resolve_token("private-reg").expect("resolve");
        assert!(tok.is_none());
    }
}
