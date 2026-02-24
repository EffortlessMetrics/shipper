//! Conversion layer from `shipper_config` model types to shared `shipper_types`.
//!
//! This crate isolates config/runtime mapping so that callers can reuse a
//! single conversion surface instead of duplicating this logic.

use shipper_config::RuntimeOptions;

/// Convert a `shipper_config::RuntimeOptions` value into `shipper_types::RuntimeOptions`.
///
/// This keeps the mapping in one place and allows downstream crates to consume a
/// stable contract without duplicating conversion logic.
pub fn into_runtime_options(value: RuntimeOptions) -> shipper_types::RuntimeOptions {
    shipper_types::RuntimeOptions {
        allow_dirty: value.allow_dirty,
        skip_ownership_check: value.skip_ownership_check,
        strict_ownership: value.strict_ownership,
        no_verify: value.no_verify,
        max_attempts: value.max_attempts,
        base_delay: value.base_delay,
        max_delay: value.max_delay,
        retry_strategy: value.retry_strategy,
        retry_jitter: value.retry_jitter,
        retry_per_error: value.retry_per_error,
        verify_timeout: value.verify_timeout,
        verify_poll_interval: value.verify_poll_interval,
        state_dir: value.state_dir,
        force_resume: value.force_resume,
        policy: value.policy,
        verify_mode: value.verify_mode,
        readiness: value.readiness,
        output_lines: value.output_lines,
        force: value.force,
        lock_timeout: value.lock_timeout,
        parallel: value.parallel,
        webhook: value.webhook,
        encryption: value.encryption,
        registries: value.registries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use shipper_config::{
        EncryptionConfig, ParallelConfig, PublishPolicy, ReadinessConfig, ReadinessMethod,
        Registry, VerifyMode, WebhookConfig,
    };
    use shipper_types as expected_types;
    use std::path::PathBuf;
    use std::time::Duration;

    fn sample_runtime_options() -> RuntimeOptions {
        RuntimeOptions {
            allow_dirty: true,
            skip_ownership_check: false,
            strict_ownership: true,
            no_verify: false,
            max_attempts: 8,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(45),
            retry_strategy: shipper_retry::RetryStrategyType::Exponential,
            retry_jitter: 0.25,
            retry_per_error: shipper_retry::PerErrorConfig::default(),
            verify_timeout: Duration::from_secs(120),
            verify_poll_interval: Duration::from_secs(3),
            state_dir: PathBuf::from("target/.shipper-tests"),
            force_resume: false,
            policy: PublishPolicy::Balanced,
            verify_mode: VerifyMode::Package,
            readiness: ReadinessConfig {
                enabled: true,
                method: ReadinessMethod::Both,
                initial_delay: Duration::from_millis(150),
                max_delay: Duration::from_secs(30),
                max_total_wait: Duration::from_secs(300),
                poll_interval: Duration::from_secs(3),
                jitter_factor: 0.4,
                index_path: Some(PathBuf::from("ci-index")),
                prefer_index: true,
            },
            output_lines: 777,
            force: true,
            lock_timeout: Duration::from_secs(4_800),
            parallel: ParallelConfig {
                enabled: true,
                max_concurrent: 6,
                per_package_timeout: Duration::from_secs(180),
            },
            webhook: WebhookConfig {
                url: "https://example.internal/webhook".to_string(),
                secret: Some("shh".to_string()),
                timeout_secs: 15,
                ..WebhookConfig::default()
            },
            encryption: EncryptionConfig {
                enabled: true,
                passphrase: Some("password".to_string()),
                env_var: Some("SHIPPER_ENCRYPT_KEY".to_string()),
            },
            registries: vec![
                Registry {
                    name: "crates-io".to_string(),
                    api_base: "https://crates.io".to_string(),
                    index_base: Some("https://index.crates.io".to_string()),
                },
                Registry {
                    name: "mirror".to_string(),
                    api_base: "https://mirror.example.local".to_string(),
                    index_base: None,
                },
            ],
        }
    }

    #[test]
    fn maps_simple_discriminants() {
        assert_eq!(PublishPolicy::Fast, expected_types::PublishPolicy::Fast);
        assert_eq!(VerifyMode::Package, expected_types::VerifyMode::Package);
        assert_eq!(
            ReadinessMethod::Index,
            expected_types::ReadinessMethod::Index
        );
    }

    #[test]
    fn maps_nested_structures_and_webhook_payload_fields() {
        let source = sample_runtime_options();
        let converted = into_runtime_options(source);

        assert_eq!(converted.policy, expected_types::PublishPolicy::Balanced);
        assert_eq!(converted.verify_mode, expected_types::VerifyMode::Package);
        assert_eq!(
            converted.readiness.method,
            expected_types::ReadinessMethod::Both
        );
        assert_eq!(converted.parallel.max_concurrent, 6);
        assert_eq!(converted.webhook.url, "https://example.internal/webhook");
        assert_eq!(converted.webhook.secret.as_deref(), Some("shh"));
        assert_eq!(converted.webhook.timeout_secs, 15);
        assert!(converted.encryption.enabled);
        assert_eq!(converted.encryption.passphrase.as_deref(), Some("password"));
        assert_eq!(converted.registries.len(), 2);
    }

    #[test]
    fn maps_readiness_config_fields() {
        let converted = sample_runtime_options().readiness;

        assert!(converted.enabled);
        assert!(converted.prefer_index);
        assert_eq!(
            converted.index_path.as_deref(),
            Some(std::path::Path::new("ci-index"))
        );
    }

    #[test]
    fn maps_parallel_config() {
        let converted = sample_runtime_options().parallel;

        assert!(converted.enabled);
        assert_eq!(converted.max_concurrent, 6);
        assert_eq!(converted.per_package_timeout, Duration::from_secs(180));
    }

    #[test]
    fn maps_registry() {
        let converted = sample_runtime_options().registries[0].clone();

        assert_eq!(converted.name, "crates-io");
        assert_eq!(converted.api_base, "https://crates.io");
    }

    fn registry_count_strategy() -> impl Strategy<Value = usize> {
        0usize..4usize
    }

    fn webhook_url_strategy() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::char::range('a', 'z'), 0..32)
            .prop_map(|chars| chars.into_iter().collect())
    }

    proptest! {
        #[test]
        fn fuzz_like_values_roundtrip_without_panic(
            allow_dirty in any::<bool>(),
            skip_ownership_check in any::<bool>(),
            strict_ownership in any::<bool>(),
            no_verify in any::<bool>(),
            max_attempts in 1u32..20,
            base_delay_ms in 0u64..5_000,
            max_delay_ms in 0u64..10_000,
            output_lines in 1usize..2000,
            policy in prop_oneof![
                Just(PublishPolicy::Safe),
                Just(PublishPolicy::Balanced),
                Just(PublishPolicy::Fast),
            ],
            verify_mode in prop_oneof![
                Just(VerifyMode::Workspace),
                Just(VerifyMode::Package),
                Just(VerifyMode::None),
            ],
            readiness_method in prop_oneof![
                Just(ReadinessMethod::Api),
                Just(ReadinessMethod::Index),
                Just(ReadinessMethod::Both),
            ],
            webhook_url in webhook_url_strategy(),
            use_secret in any::<bool>(),
            registry_count in registry_count_strategy(),
        ) {
            let webhook = WebhookConfig {
                url: webhook_url.clone(),
                secret: if use_secret { Some("secret".to_string()) } else { None },
                ..WebhookConfig::default()
            };

            let encryption = EncryptionConfig {
                enabled: true,
                passphrase: if use_secret { Some("secret-pass".to_string()) } else { None },
                ..EncryptionConfig::default()
            };

            let registries = (0..registry_count)
                .map(|idx| Registry {
                    name: format!("r-{idx}"),
                    api_base: format!("https://registry{idx}.example"),
                    index_base: Some(format!("https://registry{idx}.example/index")),
                })
                .collect();

            let input = RuntimeOptions {
                allow_dirty,
                skip_ownership_check,
                strict_ownership,
                no_verify,
                max_attempts,
                base_delay: Duration::from_millis(base_delay_ms),
                max_delay: Duration::from_millis(max_delay_ms.max(base_delay_ms + 1)),
                retry_strategy: shipper_retry::RetryStrategyType::Exponential,
                retry_jitter: 0.25,
                retry_per_error: shipper_retry::PerErrorConfig::default(),
                verify_timeout: Duration::from_secs(30),
                verify_poll_interval: Duration::from_secs(1),
                state_dir: PathBuf::from(".shipper"),
                force_resume: false,
                policy,
                verify_mode,
                readiness: ReadinessConfig {
                    enabled: true,
                    method: readiness_method,
                    initial_delay: Duration::from_millis(10),
                    max_delay: Duration::from_secs(1),
                    max_total_wait: Duration::from_secs(60),
                    poll_interval: Duration::from_millis(250),
                    jitter_factor: 0.4,
                    index_path: None,
                    prefer_index: false,
                },
                output_lines,
                force: false,
                lock_timeout: Duration::from_secs(300),
                parallel: ParallelConfig {
                    enabled: true,
                    max_concurrent: 4,
                    per_package_timeout: Duration::from_secs(120),
                },
                webhook,
                encryption,
                registries,
            };

            let converted = into_runtime_options(input);

            prop_assert_eq!(converted.allow_dirty, allow_dirty);
            prop_assert_eq!(converted.skip_ownership_check, skip_ownership_check);
            prop_assert_eq!(converted.strict_ownership, strict_ownership);
            prop_assert_eq!(converted.no_verify, no_verify);
            prop_assert_eq!(converted.max_attempts, max_attempts);
            prop_assert_eq!(converted.policy, policy);
            prop_assert_eq!(converted.verify_mode, verify_mode);
            prop_assert!(converted.readiness.enabled);
            prop_assert_eq!(converted.readiness.method, readiness_method);
            prop_assert_eq!(converted.webhook.url, webhook_url);
            prop_assert_eq!(converted.webhook.secret.is_some(), use_secret);
            prop_assert_eq!(converted.registries.len(), registry_count);
        }
    }
}
