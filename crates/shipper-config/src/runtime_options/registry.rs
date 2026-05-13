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
    } else {
        Registry {
            name: name.to_string(),
            api_base: format!("https://{name}.crates.io"),
            index_base: None,
        }
    }
}
