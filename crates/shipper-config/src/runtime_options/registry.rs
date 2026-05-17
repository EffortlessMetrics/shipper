use shipper_types::Registry;

use crate::{CliOverrides, MultiRegistryConfig, RegistryConfig};

pub(super) fn resolve(config: &MultiRegistryConfig, cli: &CliOverrides) -> Vec<Registry> {
    if cli.all_registries {
        return config
            .get_registries()
            .into_iter()
            .map(registry_from_config)
            .collect();
    }

    if let Some(ref registry_names) = cli.registries {
        return registry_names
            .iter()
            .map(|name| resolve_named_registry(config, name))
            .collect();
    }

    // Default: single registry from the plan.
    vec![]
}

fn resolve_named_registry(config: &MultiRegistryConfig, name: &str) -> Registry {
    config
        .find_by_name(name)
        .map(registry_from_config)
        .unwrap_or_else(|| default_registry_for_name(name))
}

fn registry_from_config(registry: RegistryConfig) -> Registry {
    Registry {
        name: registry.name,
        api_base: registry.api_base,
        index_base: registry.index_base,
    }
}

fn default_registry_for_name(name: &str) -> Registry {
    if name == "crates-io" {
        Registry::crates_io()
    } else if is_safe_synthetic_registry_name(name) {
        Registry {
            name: name.to_string(),
            api_base: format!("https://{name}.crates.io"),
            index_base: None,
        }
    } else {
        let mut registry = Registry::crates_io();
        registry.name = name.to_string();
        registry
    }
}

fn is_safe_synthetic_registry_name(name: &str) -> bool {
    let bytes = name.as_bytes();

    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first() != Some(&b'-')
        && bytes.last() != Some(&b'-')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::{default_registry_for_name, is_safe_synthetic_registry_name, resolve};
    use crate::{CliOverrides, MultiRegistryConfig, RegistryConfig};

    fn config_with(registries: Vec<RegistryConfig>) -> MultiRegistryConfig {
        MultiRegistryConfig {
            registries,
            default_registries: vec![],
        }
    }

    fn registry_config(name: &str) -> RegistryConfig {
        RegistryConfig {
            name: name.to_string(),
            api_base: format!("https://{name}.example/api"),
            index_base: Some(format!("https://{name}.example/index")),
            token: None,
            default: false,
        }
    }

    // ── default_registry_for_name ───────────────────────────────────────────

    #[test]
    fn safe_unknown_registry_name_uses_synthetic_crates_io_subdomain() {
        let registry = default_registry_for_name("custom-mirror");

        assert_eq!(registry.name, "custom-mirror");
        assert_eq!(registry.api_base, "https://custom-mirror.crates.io");
        assert_eq!(registry.index_base, None);
    }

    #[test]
    fn unsafe_unknown_registry_name_does_not_control_api_host() {
        let registry = default_registry_for_name("internal.example/path");

        assert_eq!(registry.name, "internal.example/path");
        assert_eq!(registry.api_base, "https://crates.io");
        assert_eq!(
            registry.index_base.as_deref(),
            Some("https://index.crates.io")
        );
    }

    #[test]
    fn unsafe_unknown_registry_name_rejects_boundary_hyphen() {
        let registry = default_registry_for_name("-custom");

        assert_eq!(registry.name, "-custom");
        assert_eq!(registry.api_base, "https://crates.io");
    }

    #[test]
    fn crates_io_name_returns_canonical_crates_io_registry() {
        let registry = default_registry_for_name("crates-io");

        assert_eq!(registry.name, "crates-io");
        assert_eq!(registry.api_base, "https://crates.io");
        assert_eq!(
            registry.index_base.as_deref(),
            Some("https://index.crates.io")
        );
    }

    // ── is_safe_synthetic_registry_name ─────────────────────────────────────

    #[test]
    fn is_safe_synthetic_registry_name_rejects_empty() {
        assert!(!is_safe_synthetic_registry_name(""));
    }

    #[test]
    fn is_safe_synthetic_registry_name_accepts_simple_lowercase() {
        assert!(is_safe_synthetic_registry_name("mirror"));
        assert!(is_safe_synthetic_registry_name("kellnr"));
    }

    #[test]
    fn is_safe_synthetic_registry_name_accepts_digits_and_internal_hyphens() {
        assert!(is_safe_synthetic_registry_name("my-mirror-7"));
        assert!(is_safe_synthetic_registry_name("mirror42"));
    }

    #[test]
    fn is_safe_synthetic_registry_name_rejects_uppercase() {
        assert!(!is_safe_synthetic_registry_name("Custom"));
    }

    #[test]
    fn is_safe_synthetic_registry_name_rejects_special_chars() {
        assert!(!is_safe_synthetic_registry_name("custom_mirror"));
        assert!(!is_safe_synthetic_registry_name("custom.mirror"));
        assert!(!is_safe_synthetic_registry_name("custom/mirror"));
    }

    #[test]
    fn is_safe_synthetic_registry_name_rejects_leading_hyphen() {
        assert!(!is_safe_synthetic_registry_name("-mirror"));
    }

    #[test]
    fn is_safe_synthetic_registry_name_rejects_trailing_hyphen() {
        assert!(!is_safe_synthetic_registry_name("mirror-"));
    }

    #[test]
    fn is_safe_synthetic_registry_name_accepts_max_length_sixty_three() {
        let max = "a".repeat(63);
        assert!(is_safe_synthetic_registry_name(&max));
    }

    #[test]
    fn is_safe_synthetic_registry_name_rejects_over_sixty_three() {
        let oversize = "a".repeat(64);
        assert!(!is_safe_synthetic_registry_name(&oversize));
    }

    // ── resolve ─────────────────────────────────────────────────────────────

    #[test]
    fn resolve_default_returns_empty_vec_so_plan_default_is_used() {
        let config = MultiRegistryConfig::default();
        let cli = CliOverrides::default();

        let result = resolve(&config, &cli);

        assert!(
            result.is_empty(),
            "no flag means use the plan's single default registry"
        );
    }

    #[test]
    fn resolve_all_registries_returns_every_configured_entry() {
        let config = config_with(vec![registry_config("alpha"), registry_config("beta")]);
        let cli = CliOverrides {
            all_registries: true,
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        let names: Vec<_> = result.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert_eq!(result[0].api_base, "https://alpha.example/api");
        assert_eq!(
            result[0].index_base.as_deref(),
            Some("https://alpha.example/index")
        );
    }

    #[test]
    fn resolve_all_registries_falls_back_to_crates_io_when_unconfigured() {
        let config = MultiRegistryConfig::default();
        let cli = CliOverrides {
            all_registries: true,
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "crates-io");
    }

    #[test]
    fn resolve_named_registry_uses_configured_when_found() {
        let config = config_with(vec![registry_config("alpha"), registry_config("beta")]);
        let cli = CliOverrides {
            registries: Some(vec!["beta".to_string()]),
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "beta");
        assert_eq!(result[0].api_base, "https://beta.example/api");
    }

    #[test]
    fn resolve_named_registry_preserves_config_index_base() {
        let config = config_with(vec![registry_config("staging")]);
        let cli = CliOverrides {
            registries: Some(vec!["staging".to_string()]),
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "staging");
        assert_eq!(
            result[0].index_base.as_deref(),
            Some("https://staging.example/index")
        );
    }

    #[test]
    fn resolve_named_registry_falls_back_to_synthetic_default_when_unknown_safe() {
        let config = MultiRegistryConfig::default();
        let cli = CliOverrides {
            registries: Some(vec!["my-mirror".to_string()]),
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "my-mirror");
        assert_eq!(result[0].api_base, "https://my-mirror.crates.io");
        assert!(result[0].index_base.is_none());
    }

    #[test]
    fn resolve_named_registry_falls_back_to_crates_io_when_unknown_unsafe() {
        let config = MultiRegistryConfig::default();
        let cli = CliOverrides {
            registries: Some(vec!["DANGER/registry".to_string()]),
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "DANGER/registry");
        assert_eq!(result[0].api_base, "https://crates.io");
    }

    #[test]
    fn resolve_named_registry_preserves_request_order() {
        let config = config_with(vec![registry_config("alpha"), registry_config("beta")]);
        let cli = CliOverrides {
            registries: Some(vec!["beta".to_string(), "alpha".to_string()]),
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        let names: Vec<_> = result.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "alpha"]);
    }

    #[test]
    fn resolve_all_registries_takes_precedence_over_named() {
        let config = config_with(vec![registry_config("alpha"), registry_config("beta")]);
        let cli = CliOverrides {
            all_registries: true,
            registries: Some(vec!["beta".to_string()]),
            ..CliOverrides::default()
        };

        let result = resolve(&config, &cli);

        let names: Vec<_> = result.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
