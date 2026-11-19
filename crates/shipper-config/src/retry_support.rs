use anyhow::{Result, bail};
use shipper_retry::PerErrorConfig;

/// Validate the supported per-error retry configuration at an execution boundary.
///
/// Class policies remain parseable for diagnostics and migration, but the engine
/// does not execute them. Config-file validation and direct engine entry points
/// use this same check before effects; an omitted or empty configuration is valid.
pub fn validate_per_error_config(config: &PerErrorConfig) -> Result<()> {
    if config.retryable.is_some() || config.ambiguous.is_some() || config.permanent.is_some() {
        bail!(
            "retry.per_error is not supported; remove its class entries and configure the global [retry] policy instead"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CliOverrides, ShipperConfig};

    #[test]
    fn per_error_refuses_every_configured_class_including_empty_class_tables() {
        for class in ["retryable", "ambiguous", "permanent"] {
            for fields in ["", "max_attempts = 2\nbase_delay = \"1s\"\n"] {
                let config: ShipperConfig =
                    toml::from_str(&format!("[retry.per_error.{class}]\n{fields}"))
                        .expect("known class remains parseable");
                let error = config.validate().expect_err("unsupported class policy");
                assert!(
                    error
                        .to_string()
                        .contains("retry.per_error is not supported")
                );

                // CLI retry flags cannot turn an unsupported class into an
                // applied policy or silently clear it during resolution.
                let opts = config.build_runtime_options(CliOverrides {
                    max_attempts: Some(9),
                    ..Default::default()
                });
                assert_eq!(opts.max_attempts, 9);
                assert!(validate_per_error_config(&opts.retry_per_error).is_err());
            }
        }
    }

    #[test]
    fn per_error_omitted_and_empty_preserve_global_policy_resolution() {
        for suffix in ["", "[retry.per_error]\n", "[retry.per_error] # empty\n"] {
            let config: ShipperConfig = toml::from_str(&format!(
                "[retry]\npolicy = \"custom\"\nmax_attempts = 3\nbase_delay = \"1s\"\nmax_delay = \"4s\"\njitter = 0.0\n{suffix}"
            ))
            .expect("parse global configuration");
            config.validate().expect("supported global configuration");
            let opts = config.build_runtime_options(CliOverrides::default());
            assert_eq!(opts.max_attempts, 3);
            assert_eq!(opts.base_delay, std::time::Duration::from_secs(1));
            assert_eq!(opts.max_delay, std::time::Duration::from_secs(4));
            assert_eq!(opts.retry_jitter, 0.0);
            validate_per_error_config(&opts.retry_per_error).expect("empty class configuration");
        }
    }

    #[test]
    fn per_error_unknown_keys_cannot_disappear_into_an_empty_configuration() {
        for input in [
            "[retry.per_error]\nmax_attempts = 2\n",
            "[retry.per_error.retriable]\nmax_attempts = 2\n",
        ] {
            let error = toml::from_str::<ShipperConfig>(input).expect_err("unknown class key");
            assert!(error.to_string().contains("unknown field"));
        }
    }

    #[test]
    #[serial_test::serial]
    fn per_error_validated_load_refuses_while_diagnostic_load_preserves_class_values() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(".shipper.toml");
        std::fs::write(&path, "[retry.per_error.retryable]\nbase_delay = \"1s\"\n")
            .expect("write config");
        let error = ShipperConfig::load_from_file(&path).expect_err("refuse unsupported config");
        assert!(format!("{error:#}").contains("retry.per_error is not supported"));
        let diagnostic = ShipperConfig::load_from_file_for_diagnostics(&path)
            .expect("diagnostic loading remains available");
        // Preserve the historical serde value rather than activating or
        // normalizing it: omitted nested max_attempts is 0, Default is 6.
        assert_eq!(
            diagnostic
                .retry
                .per_error
                .retryable
                .as_ref()
                .unwrap()
                .max_attempts,
            0
        );
        assert!(diagnostic.validate().is_err());
    }

    #[test]
    fn per_error_template_does_not_advertise_unsupported_classes() {
        let template = ShipperConfig::default_toml_template();
        assert!(!template.contains("[retry.per_error."));
        let config: ShipperConfig = toml::from_str(&template).expect("parse template");
        config.validate().expect("template must remain supported");
    }
}
