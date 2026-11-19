use std::collections::BTreeMap;

use shipper_types::{Registry, RegistryTrustOptions};

use crate::{CliOverrides, MultiRegistryConfig, RegistryConfig, RehearsalConfig};

#[cfg(test)]
pub(super) fn resolve(config: &MultiRegistryConfig, cli: &CliOverrides) -> Vec<Registry> {
    resolve_with_policies(config, None, &RehearsalConfig::default(), cli).0
}

pub(super) fn resolve_with_policies(
    config: &MultiRegistryConfig,
    single_registry: Option<&RegistryConfig>,
    rehearsal: &RehearsalConfig,
    cli: &CliOverrides,
) -> (Vec<Registry>, BTreeMap<String, RegistryTrustOptions>) {
    if cli.all_registries {
        let registries = config
            .get_registries()
            .into_iter()
            .map(registry_from_config)
            .collect();
        let mut policies = config
            .get_registries()
            .into_iter()
            .map(|registry| {
                let allow_loopback = cli
                    .registry_name
                    .as_deref()
                    .is_some_and(|name| cli.allow_loopback && name == registry.name)
                    || rehearsal_allows_loopback(rehearsal, &registry.name);
                (
                    registry.name,
                    RegistryTrustOptions {
                        allow_private: registry.allow_private,
                        allow_loopback,
                    },
                )
            })
            .collect();
        preserve_single_registry_policy(&mut policies, single_registry, rehearsal, cli);
        return (registries, policies);
    }

    if let Some(ref registry_names) = cli.registries {
        let resolved = registry_names
            .iter()
            .map(|name| resolve_named_registry(config, name))
            .collect::<Vec<_>>();
        let mut policies = registry_names
            .iter()
            .map(|name| {
                let configured = config.find_by_name(name);
                (
                    name.clone(),
                    RegistryTrustOptions {
                        allow_private: configured.is_some_and(|registry| registry.allow_private),
                        allow_loopback: cli.allow_loopback
                            || rehearsal_allows_loopback(rehearsal, name),
                    },
                )
            })
            .collect();
        preserve_single_registry_policy(&mut policies, single_registry, rehearsal, cli);
        return (resolved, policies);
    }

    // Default: single registry from the plan. Carry policy for the legacy
    // single-registry configuration too; the CLI may override its identity
    // and URL while retaining the explicitly configured trust posture.
    let policies = if let Some(registry) = single_registry {
        let mut policies = BTreeMap::new();
        let effective_name = cli
            .registry_name
            .as_deref()
            .unwrap_or(registry.name.as_str());
        policies.insert(
            effective_name.to_string(),
            RegistryTrustOptions {
                allow_private: registry.allow_private,
                allow_loopback: cli.allow_loopback
                    || rehearsal_allows_loopback(rehearsal, effective_name),
            },
        );
        policies
    } else if cli.allow_loopback {
        let mut policies = BTreeMap::new();
        let registry_name = cli
            .registry_name
            .as_deref()
            .unwrap_or("crates-io")
            .to_string();
        policies.insert(
            registry_name,
            RegistryTrustOptions {
                allow_private: false,
                allow_loopback: true,
            },
        );
        policies
    } else {
        BTreeMap::new()
    };
    (vec![], policies)
}

fn preserve_single_registry_policy(
    policies: &mut BTreeMap<String, RegistryTrustOptions>,
    single_registry: Option<&RegistryConfig>,
    rehearsal: &RehearsalConfig,
    cli: &CliOverrides,
) {
    let Some(registry) = single_registry else {
        return;
    };
    let effective_name = cli
        .registry_name
        .as_deref()
        .unwrap_or(registry.name.as_str());
    policies
        .entry(effective_name.to_string())
        .or_insert(RegistryTrustOptions {
            allow_private: registry.allow_private,
            allow_loopback: cli.allow_loopback
                || rehearsal_allows_loopback(rehearsal, effective_name),
        });
}

fn rehearsal_allows_loopback(rehearsal: &RehearsalConfig, registry_name: &str) -> bool {
    (rehearsal.enabled || rehearsal.allow_loopback)
        && rehearsal.registry.as_deref() == Some(registry_name)
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
    use super::{
        default_registry_for_name, is_safe_synthetic_registry_name, rehearsal_allows_loopback,
        resolve, resolve_with_policies,
    };
    use crate::{CliOverrides, MultiRegistryConfig, RegistryConfig, RehearsalConfig};

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
            allow_private: false,
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
    fn resolve_carries_private_registry_opt_in_separately_from_identity() {
        let mut private = registry_config("private");
        private.allow_private = true;
        let config = config_with(vec![private]);
        let cli = CliOverrides {
            all_registries: true,
            ..CliOverrides::default()
        };

        let (registries, policies) =
            resolve_with_policies(&config, None, &RehearsalConfig::default(), &cli);

        assert_eq!(registries[0].name, "private");
        assert!(policies["private"].allow_private);
        assert!(!policies["private"].allow_loopback);
    }

    #[test]
    fn resolve_all_registries_scopes_cli_loopback_to_selected_registry() {
        let config = config_with(vec![registry_config("alpha"), registry_config("beta")]);
        let cli = CliOverrides {
            all_registries: true,
            allow_loopback: true,
            registry_name: Some("alpha".to_string()),
            ..CliOverrides::default()
        };

        let (_, policies) = resolve_with_policies(&config, None, &RehearsalConfig::default(), &cli);

        assert!(policies["alpha"].allow_loopback);
        assert!(!policies["beta"].allow_loopback);
    }

    #[test]
    fn resolve_default_carries_single_registry_rehearsal_posture() {
        let registry = registry_config("local");
        let config = config_with(vec![]);
        let rehearsal = RehearsalConfig {
            enabled: true,
            allow_loopback: false,
            registry: Some("local".to_string()),
        };
        let (_, policies) = resolve_with_policies(
            &config,
            Some(&registry),
            &rehearsal,
            &CliOverrides::default(),
        );

        assert!(policies["local"].allow_loopback);
        assert!(!policies["local"].allow_private);
    }

    #[test]
    fn resolve_named_registry_combines_cli_and_rehearsal_loopback_policy() {
        let config = config_with(vec![registry_config("local")]);
        let cli_only = CliOverrides {
            registries: Some(vec!["local".to_string()]),
            allow_loopback: true,
            ..CliOverrides::default()
        };
        let (_, policies) =
            resolve_with_policies(&config, None, &RehearsalConfig::default(), &cli_only);
        assert!(policies["local"].allow_loopback);

        let rehearsal_only = RehearsalConfig {
            enabled: false,
            allow_loopback: true,
            registry: Some("local".to_string()),
        };
        let cli = CliOverrides {
            registries: Some(vec!["local".to_string()]),
            ..CliOverrides::default()
        };
        let (_, policies) = resolve_with_policies(&config, None, &rehearsal_only, &cli);
        assert!(policies["local"].allow_loopback);
    }

    #[test]
    fn resolve_named_rehearsal_preserves_live_policy_without_selecting_live() {
        let config = config_with(vec![registry_config("rehearsal")]);
        let mut live = registry_config("live");
        live.allow_private = true;
        let rehearsal = RehearsalConfig {
            enabled: true,
            allow_loopback: true,
            registry: Some("rehearsal".to_string()),
        };
        let cli = CliOverrides {
            registries: Some(vec!["rehearsal".to_string()]),
            allow_loopback: true,
            rehearsal_registry: Some("rehearsal".to_string()),
            ..CliOverrides::default()
        };

        let (registries, policies) = resolve_with_policies(&config, Some(&live), &rehearsal, &cli);

        assert_eq!(
            registries
                .iter()
                .map(|registry| registry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["rehearsal"],
            "live trust metadata must not make the live target dispatchable"
        );
        assert!(policies["rehearsal"].allow_loopback);
        assert!(policies["live"].allow_loopback);
        assert!(policies["live"].allow_private);
    }

    #[test]
    fn rehearsal_loopback_policy_covers_both_flags_and_registry_identity() {
        let cases = [
            (false, false, "local", false),
            (true, false, "local", true),
            (false, true, "local", true),
            (true, true, "other", false),
        ];

        for (enabled, allow_loopback, registry_name, expected) in cases {
            let rehearsal = RehearsalConfig {
                enabled,
                allow_loopback,
                registry: Some("local".to_string()),
            };
            assert_eq!(
                rehearsal_allows_loopback(&rehearsal, registry_name),
                expected,
                "enabled={enabled}, allow_loopback={allow_loopback}, registry={registry_name}"
            );
        }
    }

    #[test]
    fn resolve_cli_loopback_opt_in_is_explicit_and_does_not_allow_private() {
        let registry = registry_config("local");
        let cli = CliOverrides {
            allow_loopback: true,
            ..CliOverrides::default()
        };

        let (_, policies) = resolve_with_policies(
            &MultiRegistryConfig::default(),
            Some(&registry),
            &RehearsalConfig::default(),
            &cli,
        );

        assert!(policies["local"].allow_loopback);
        assert!(!policies["local"].allow_private);
    }

    #[test]
    fn resolve_cli_loopback_opt_in_tracks_cli_selected_registry() {
        let cli = CliOverrides {
            allow_loopback: true,
            registry_name: Some("local".to_string()),
            ..CliOverrides::default()
        };

        let (_, policies) = resolve_with_policies(
            &MultiRegistryConfig::default(),
            None,
            &RehearsalConfig::default(),
            &cli,
        );

        assert!(policies["local"].allow_loopback);
        assert!(!policies["local"].allow_private);
    }

    #[test]
    fn resolve_single_registry_rekeys_policy_for_cli_selected_registry() {
        let mut registry = registry_config("configured-registry");
        registry.allow_private = true;
        let cli = CliOverrides {
            registry_name: Some("selected-registry".to_string()),
            ..CliOverrides::default()
        };

        let (_, policies) = resolve_with_policies(
            &MultiRegistryConfig::default(),
            Some(&registry),
            &RehearsalConfig::default(),
            &cli,
        );

        assert_eq!(policies.len(), 1);
        assert!(policies["selected-registry"].allow_private);
        assert!(!policies.contains_key("configured-registry"));
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
