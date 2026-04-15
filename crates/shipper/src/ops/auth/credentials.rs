//! `$CARGO_HOME/credentials.toml` parsing.
//!
//! Understands the standard formats:
//!   - `[registry] token = "..."` (legacy crates.io)
//!   - `[registries.<name>] token = "..."`
//!   - Top-level `token = "..."` (oldest format)
//!
//! The `_extended` helper additionally handles crates.io aliases
//! (`crates.io`, `crates_io`, and the nested `[registries.crates.io]` form)
//! needed to keep parity with Cargo's more forgiving lookup.

use std::path::Path;

use anyhow::{Context, Result};

use super::resolver::CRATES_IO_REGISTRY;

/// Default credentials filename inside `$CARGO_HOME`.
pub const CREDENTIALS_FILE: &str = "credentials.toml";

/// Read a token from a credentials file, using the strict cargo layout.
///
/// Returns `Err` when the file does not exist, is malformed, or contains no
/// token entry for `registry`.
pub(crate) fn token_from_credentials_file(path: &Path, registry: &str) -> Result<String> {
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

/// Extended credentials-file lookup that returns `Ok(None)` instead of `Err`
/// when the registry simply has no token entry, and also handles crates.io
/// aliases (`crates.io`, `crates_io`, nested `[registries.crates.io]`).
///
/// This is used by the crate-public [`super::resolve_token`] fallback which
/// walks both `credentials.toml` and the legacy `credentials` filename.
pub(crate) fn token_from_credentials_file_extended(
    path: &Path,
    registry_name: &str,
) -> Result<Option<String>> {
    let content = std::fs::read_to_string(path).with_context(|| {
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

/// List all registry names found in a `credentials.toml` file.
///
/// Returns an empty `Vec` when the file does not exist. Propagates errors
/// for I/O or TOML parse failures.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
    fn credentials_file_malformed_toml() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "this is not valid toml [[[").expect("write");

        let result = token_from_credentials_file(&path, CRATES_IO_REGISTRY);
        assert!(result.is_err());
    }

    #[test]
    fn credentials_file_simple_key_value_format() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "token = \"legacy-token\"\n").expect("write");

        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "legacy-token");
    }

    #[test]
    fn credentials_file_registry_section_takes_priority_over_simple_key() {
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
    fn credentials_file_registry_section_over_registries_for_crates_io() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        let content = r#"
[registry]
token = "registry-section-token"

[registries.crates-io]
token = "registries-crates-io-token"
"#;
        std::fs::write(&path, content).expect("write");
        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "registry-section-token");
    }

    #[test]
    fn credentials_file_registries_crates_io_fallback() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        let content = r#"
[registries.crates-io]
token = "registries-crates-io-token"
"#;
        std::fs::write(&path, content).expect("write");
        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "registries-crates-io-token");
    }

    #[test]
    fn credentials_file_whitespace_only_token() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "[registry]\ntoken = \"   \"\n").expect("write");
        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "   ");
    }

    #[test]
    fn credentials_file_very_long_token() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        let long_token = "a".repeat(10_000);
        let content = format!("[registry]\ntoken = \"{long_token}\"\n");
        std::fs::write(&path, &content).expect("write");
        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token.len(), 10_000);
    }

    #[test]
    fn credentials_file_token_with_unicode() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "[registry]\ntoken = \"tök€n_πλ∞\"\n").expect("write");
        let token = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
        assert_eq!(token, "tök€n_πλ∞");
    }

    #[test]
    fn credentials_file_is_directory_not_file() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CREDENTIALS_FILE);
        std::fs::create_dir(&path).expect("create dir");
        let result = token_from_credentials_file(&path, CRATES_IO_REGISTRY);
        assert!(result.is_err());
    }

    // ── snapshot tests ──────────────────────────────────────────────────

    mod snapshots {
        use super::*;
        use insta::assert_debug_snapshot;
        use tempfile::tempdir;

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
        fn snapshot_list_registries_many() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            let content = r#"
[registry]
token = "default-token"

[registries.alpha]
token = "a"

[registries.beta]
token = "b"

[registries.gamma]
token = "c"

[registries.delta]
token = "d"
"#;
            std::fs::write(&path, content).expect("write");
            let mut registries = list_configured_registries(&path).unwrap();
            registries.sort();
            assert_debug_snapshot!(registries);
        }

        #[test]
        fn snapshot_credentials_file_with_all_formats() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            let content = r#"
token = "legacy-top-level"

[registry]
token = "registry-section"

[registries.custom]
token = "custom-registry"
"#;
            std::fs::write(&path, content).expect("write");
            let crates_io = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
            let custom = token_from_credentials_file(&path, "custom").unwrap();
            assert_debug_snapshot!((crates_io, custom));
        }
    }

    // ── error message snapshots ──────────────────────────────────────────

    mod error_message_snapshots {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn snapshot_error_credentials_file_not_found_message() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join("nonexistent_credentials.toml");
            let err = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap_err();
            insta::assert_snapshot!("error_msg_credentials_not_found", err.to_string());
        }

        #[test]
        fn snapshot_error_malformed_toml_message() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&path, "this is [[[not valid toml").expect("write");
            let err = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap_err();
            insta::assert_snapshot!("error_msg_malformed_toml", err.root_cause().to_string());
        }

        #[test]
        fn snapshot_error_token_not_found_for_wrong_registry() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&path, "[registry]\ntoken = \"my-tok\"\n").expect("write");
            let err =
                token_from_credentials_file(&path, "nonexistent-private-registry").unwrap_err();
            insta::assert_snapshot!("error_msg_token_wrong_registry", err.to_string());
        }

        #[test]
        fn snapshot_error_empty_credentials_no_token() {
            let td = tempdir().expect("tempdir");
            let path = td.path().join(CREDENTIALS_FILE);
            std::fs::write(&path, "# empty credentials file\n").expect("write");
            let err = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap_err();
            insta::assert_snapshot!("error_msg_empty_credentials", err.to_string());
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
            fn token_roundtrip_via_credentials_file(token in token_strategy()) {
                let td = tempfile::tempdir().expect("tempdir");
                let path = td.path().join(CREDENTIALS_FILE);
                let content = format!("[registry]\ntoken = \"{token}\"\n");
                std::fs::write(&path, &content).expect("write");

                let result = token_from_credentials_file(&path, CRATES_IO_REGISTRY).unwrap();
                prop_assert_eq!(result, token);
            }

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

            #[test]
            fn list_registries_never_panics(content in "\\PC{0,200}") {
                let td = tempfile::tempdir().expect("tempdir");
                let path = td.path().join(CREDENTIALS_FILE);
                std::fs::write(&path, &content).expect("write");
                let _ = list_configured_registries(&path);
            }
        }
    }
}
