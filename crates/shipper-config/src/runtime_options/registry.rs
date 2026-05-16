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
    use super::default_registry_for_name;

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

    use super::{is_safe_synthetic_registry_name, resolve, resolve_named_registry};
    use crate::{CliOverrides, MultiRegistryConfig, RegistryConfig};

    fn empty_cli() -> CliOverrides {
        CliOverrides::default()
    }

    fn config_with_registries(registries: Vec<RegistryConfig>) -> MultiRegistryConfig {
        MultiRegistryConfig {
            registries,
            ..MultiRegistryConfig::default()
        }
    }

    fn registry_config(name: &str, api_base: &str) -> RegistryConfig {
        RegistryConfig {
            name: name.to_string(),
            api_base: api_base.to_string(),
            index_base: None,
            token: None,
            default: false,
        }
    }

    #[test]
    fn resolve_no_cli_flags_returns_empty_vec() {
        let config = config_with_registries(vec![registry_config(
            "internal",
            "https://internal.example.com",
        )]);

        let resolved = resolve(&config, &empty_cli());

        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_all_registries_returns_every_configured_registry() {
        let config = config_with_registries(vec![
            registry_config("first", "https://first.example.com"),
            registry_config("second", "https://second.example.com"),
        ]);

        let mut cli = empty_cli();
        cli.all_registries = true;

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "first");
        assert_eq!(resolved[0].api_base, "https://first.example.com");
        assert_eq!(resolved[1].name, "second");
        assert_eq!(resolved[1].api_base, "https://second.example.com");
    }

    #[test]
    fn resolve_all_registries_falls_back_to_crates_io_when_none_configured() {
        let config = MultiRegistryConfig::default();

        let mut cli = empty_cli();
        cli.all_registries = true;

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "crates-io");
        assert_eq!(resolved[0].api_base, "https://crates.io");
    }

    #[test]
    fn resolve_named_registries_resolves_each_by_name() {
        let config = config_with_registries(vec![
            registry_config("alpha", "https://alpha.example.com"),
            registry_config("beta", "https://beta.example.com"),
        ]);

        let mut cli = empty_cli();
        cli.registries = Some(vec!["alpha".to_string(), "beta".to_string()]);

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "alpha");
        assert_eq!(resolved[1].name, "beta");
    }

    #[test]
    fn resolve_named_registries_falls_back_to_default_for_unknown_name() {
        let config = MultiRegistryConfig::default();

        let mut cli = empty_cli();
        cli.registries = Some(vec!["crates-io".to_string()]);

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "crates-io");
        assert_eq!(resolved[0].api_base, "https://crates.io");
    }

    #[test]
    fn resolve_named_registries_uses_synthetic_subdomain_for_safe_unknown_name() {
        let config = MultiRegistryConfig::default();

        let mut cli = empty_cli();
        cli.registries = Some(vec!["company-mirror".to_string()]);

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "company-mirror");
        assert_eq!(resolved[0].api_base, "https://company-mirror.crates.io");
    }

    #[test]
    fn resolve_all_registries_takes_precedence_over_named_registries() {
        let config = config_with_registries(vec![
            registry_config("a", "https://a.example.com"),
            registry_config("b", "https://b.example.com"),
        ]);

        let mut cli = empty_cli();
        cli.all_registries = true;
        cli.registries = Some(vec!["a".to_string()]);

        let resolved = resolve(&config, &cli);

        // all_registries wins → both registries returned, ignoring `--registries a`.
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn resolve_named_registry_uses_config_index_base_when_present() {
        let mut entry = registry_config("staging", "https://staging.example.com");
        entry.index_base = Some("https://index.staging.example.com".to_string());
        let config = config_with_registries(vec![entry]);

        let resolved = resolve_named_registry(&config, "staging");

        assert_eq!(resolved.name, "staging");
        assert_eq!(
            resolved.index_base.as_deref(),
            Some("https://index.staging.example.com")
        );
    }

    #[test]
    fn resolve_named_registry_unknown_falls_back_to_default_branch() {
        let config = MultiRegistryConfig::default();

        let resolved = resolve_named_registry(&config, "crates-io");

        assert_eq!(resolved.name, "crates-io");
        assert_eq!(resolved.api_base, "https://crates.io");
    }

    #[test]
    fn is_safe_synthetic_registry_name_accepts_valid_names() {
        assert!(is_safe_synthetic_registry_name("foo"));
        assert!(is_safe_synthetic_registry_name("foo-bar"));
        assert!(is_safe_synthetic_registry_name("a1b2c3"));
        assert!(is_safe_synthetic_registry_name("a"));
        assert!(is_safe_synthetic_registry_name(&"a".repeat(63)));
    }

    #[test]
    fn is_safe_synthetic_registry_name_rejects_invalid_names() {
        assert!(!is_safe_synthetic_registry_name(""));
        assert!(!is_safe_synthetic_registry_name("-leading-hyphen"));
        assert!(!is_safe_synthetic_registry_name("trailing-hyphen-"));
        assert!(!is_safe_synthetic_registry_name("UPPER"));
        assert!(!is_safe_synthetic_registry_name("has.dot"));
        assert!(!is_safe_synthetic_registry_name("has/slash"));
        assert!(!is_safe_synthetic_registry_name("has space"));
        assert!(!is_safe_synthetic_registry_name(&"a".repeat(64)));
    }
}
