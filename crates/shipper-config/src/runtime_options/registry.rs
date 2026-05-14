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
}
