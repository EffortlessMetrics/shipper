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
        if path.exists() {
            if let Some(tok) = token_from_credentials_file(&path, registry_name)? {
                return Ok(Some(tok));
            }
        }
    }

    Ok(None)
}

fn token_from_env(registry_name: &str) -> Option<String> {
    // The default registry (crates.io) has a special env var name.
    if registry_name == "crates-io" {
        if let Ok(v) = env::var("CARGO_REGISTRY_TOKEN") {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }

    let env_name = format!("CARGO_REGISTRIES_{}_TOKEN", normalize_registry_for_env(registry_name));
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
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read cargo credentials file at {}", path.display()))?;

    let value: toml::Value = content.parse().with_context(|| {
        format!(
            "failed to parse cargo credentials file as TOML: {}",
            path.display()
        )
    })?;

    // crates.io commonly uses `[registry] token = "..."`.
    if registry_name == "crates-io" {
        if let Some(tok) = value
            .get("registry")
            .and_then(|t| t.get("token"))
            .and_then(|v| v.as_str())
        {
            let tok = tok.trim().to_string();
            if !tok.is_empty() {
                return Ok(Some(tok));
            }
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
