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
        resume_from: value.resume_from,
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
            resume_from: Some("my-crate".to_string()),
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
                resume_from: None,
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

    // ── Edge case: empty registries list ──────────────────────────────
    #[test]
    fn empty_registries_list() {
        let mut opts = sample_runtime_options();
        opts.registries = vec![];
        let converted = into_runtime_options(opts);
        assert!(converted.registries.is_empty());
    }

    // ── Edge case: None optionals ──────────────────────────────────────
    #[test]
    fn none_webhook_secret() {
        let mut opts = sample_runtime_options();
        opts.webhook.secret = None;
        let converted = into_runtime_options(opts);
        assert!(converted.webhook.secret.is_none());
    }

    #[test]
    fn none_encryption_passphrase() {
        let mut opts = sample_runtime_options();
        opts.encryption.passphrase = None;
        let converted = into_runtime_options(opts);
        assert!(converted.encryption.passphrase.is_none());
    }

    #[test]
    fn none_encryption_env_var() {
        let mut opts = sample_runtime_options();
        opts.encryption.env_var = None;
        let converted = into_runtime_options(opts);
        assert!(converted.encryption.env_var.is_none());
    }

    #[test]
    fn none_resume_from() {
        let mut opts = sample_runtime_options();
        opts.resume_from = None;
        let converted = into_runtime_options(opts);
        assert!(converted.resume_from.is_none());
    }

    #[test]
    fn none_index_path() {
        let mut opts = sample_runtime_options();
        opts.readiness.index_path = None;
        let converted = into_runtime_options(opts);
        assert!(converted.readiness.index_path.is_none());
    }

    #[test]
    fn none_index_base_in_registry() {
        let mut opts = sample_runtime_options();
        opts.registries = vec![Registry {
            name: "test".to_string(),
            api_base: "https://example.com".to_string(),
            index_base: None,
        }];
        let converted = into_runtime_options(opts);
        assert!(converted.registries[0].index_base.is_none());
    }

    // ── Duration boundary cases ────────────────────────────────────────
    #[test]
    fn zero_duration_base_delay() {
        let mut opts = sample_runtime_options();
        opts.base_delay = Duration::ZERO;
        let converted = into_runtime_options(opts);
        assert_eq!(converted.base_delay, Duration::ZERO);
    }

    #[test]
    fn zero_duration_max_delay() {
        let mut opts = sample_runtime_options();
        opts.max_delay = Duration::ZERO;
        let converted = into_runtime_options(opts);
        assert_eq!(converted.max_delay, Duration::ZERO);
    }

    #[test]
    fn zero_duration_verify_timeout() {
        let mut opts = sample_runtime_options();
        opts.verify_timeout = Duration::ZERO;
        let converted = into_runtime_options(opts);
        assert_eq!(converted.verify_timeout, Duration::ZERO);
    }

    #[test]
    fn zero_duration_lock_timeout() {
        let mut opts = sample_runtime_options();
        opts.lock_timeout = Duration::ZERO;
        let converted = into_runtime_options(opts);
        assert_eq!(converted.lock_timeout, Duration::ZERO);
    }

    #[test]
    fn very_small_duration_one_nanosecond() {
        let mut opts = sample_runtime_options();
        opts.base_delay = Duration::from_nanos(1);
        opts.verify_poll_interval = Duration::from_nanos(1);
        let converted = into_runtime_options(opts);
        assert_eq!(converted.base_delay, Duration::from_nanos(1));
        assert_eq!(converted.verify_poll_interval, Duration::from_nanos(1));
    }

    #[test]
    fn very_large_duration() {
        let large = Duration::from_secs(u64::MAX / 2);
        let mut opts = sample_runtime_options();
        opts.max_delay = large;
        opts.lock_timeout = large;
        let converted = into_runtime_options(opts);
        assert_eq!(converted.max_delay, large);
        assert_eq!(converted.lock_timeout, large);
    }

    #[test]
    fn sub_millisecond_readiness_delays() {
        let mut opts = sample_runtime_options();
        opts.readiness.initial_delay = Duration::from_micros(500);
        opts.readiness.poll_interval = Duration::from_micros(100);
        let converted = into_runtime_options(opts);
        assert_eq!(converted.readiness.initial_delay, Duration::from_micros(500));
        assert_eq!(converted.readiness.poll_interval, Duration::from_micros(100));
    }

    #[test]
    fn zero_per_package_timeout() {
        let mut opts = sample_runtime_options();
        opts.parallel.per_package_timeout = Duration::ZERO;
        let converted = into_runtime_options(opts);
        assert_eq!(converted.parallel.per_package_timeout, Duration::ZERO);
    }

    // ── Individual field mapping ───────────────────────────────────────
    #[test]
    fn maps_allow_dirty() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.allow_dirty = val;
            assert_eq!(into_runtime_options(opts).allow_dirty, val);
        }
    }

    #[test]
    fn maps_skip_ownership_check() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.skip_ownership_check = val;
            assert_eq!(into_runtime_options(opts).skip_ownership_check, val);
        }
    }

    #[test]
    fn maps_strict_ownership() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.strict_ownership = val;
            assert_eq!(into_runtime_options(opts).strict_ownership, val);
        }
    }

    #[test]
    fn maps_no_verify() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.no_verify = val;
            assert_eq!(into_runtime_options(opts).no_verify, val);
        }
    }

    #[test]
    fn maps_max_attempts() {
        let mut opts = sample_runtime_options();
        opts.max_attempts = 42;
        assert_eq!(into_runtime_options(opts).max_attempts, 42);
    }

    #[test]
    fn maps_base_delay() {
        let mut opts = sample_runtime_options();
        opts.base_delay = Duration::from_millis(999);
        assert_eq!(into_runtime_options(opts).base_delay, Duration::from_millis(999));
    }

    #[test]
    fn maps_max_delay() {
        let mut opts = sample_runtime_options();
        opts.max_delay = Duration::from_secs(9999);
        assert_eq!(into_runtime_options(opts).max_delay, Duration::from_secs(9999));
    }

    #[test]
    fn maps_retry_strategy() {
        for strategy in [
            shipper_retry::RetryStrategyType::Immediate,
            shipper_retry::RetryStrategyType::Exponential,
            shipper_retry::RetryStrategyType::Linear,
            shipper_retry::RetryStrategyType::Constant,
        ] {
            let mut opts = sample_runtime_options();
            opts.retry_strategy = strategy;
            assert_eq!(into_runtime_options(opts).retry_strategy, strategy);
        }
    }

    #[test]
    fn maps_retry_jitter() {
        let mut opts = sample_runtime_options();
        opts.retry_jitter = 0.99;
        let converted = into_runtime_options(opts);
        assert!((converted.retry_jitter - 0.99).abs() < f64::EPSILON);
    }

    #[test]
    fn maps_verify_timeout() {
        let mut opts = sample_runtime_options();
        opts.verify_timeout = Duration::from_secs(555);
        assert_eq!(into_runtime_options(opts).verify_timeout, Duration::from_secs(555));
    }

    #[test]
    fn maps_verify_poll_interval() {
        let mut opts = sample_runtime_options();
        opts.verify_poll_interval = Duration::from_millis(750);
        assert_eq!(
            into_runtime_options(opts).verify_poll_interval,
            Duration::from_millis(750)
        );
    }

    #[test]
    fn maps_state_dir() {
        let mut opts = sample_runtime_options();
        opts.state_dir = PathBuf::from("/tmp/custom-state");
        assert_eq!(
            into_runtime_options(opts).state_dir,
            PathBuf::from("/tmp/custom-state")
        );
    }

    #[test]
    fn maps_force_resume() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.force_resume = val;
            assert_eq!(into_runtime_options(opts).force_resume, val);
        }
    }

    #[test]
    fn maps_policy_variants() {
        for policy in [PublishPolicy::Safe, PublishPolicy::Balanced, PublishPolicy::Fast] {
            let mut opts = sample_runtime_options();
            opts.policy = policy;
            assert_eq!(into_runtime_options(opts).policy, policy);
        }
    }

    #[test]
    fn maps_verify_mode_variants() {
        for mode in [VerifyMode::Workspace, VerifyMode::Package, VerifyMode::None] {
            let mut opts = sample_runtime_options();
            opts.verify_mode = mode;
            assert_eq!(into_runtime_options(opts).verify_mode, mode);
        }
    }

    #[test]
    fn maps_output_lines() {
        let mut opts = sample_runtime_options();
        opts.output_lines = 0;
        assert_eq!(into_runtime_options(opts).output_lines, 0);
    }

    #[test]
    fn maps_force() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.force = val;
            assert_eq!(into_runtime_options(opts).force, val);
        }
    }

    #[test]
    fn maps_lock_timeout() {
        let mut opts = sample_runtime_options();
        opts.lock_timeout = Duration::from_secs(12345);
        assert_eq!(into_runtime_options(opts).lock_timeout, Duration::from_secs(12345));
    }

    #[test]
    fn maps_resume_from_some() {
        let mut opts = sample_runtime_options();
        opts.resume_from = Some("specific-crate".to_string());
        assert_eq!(
            into_runtime_options(opts).resume_from.as_deref(),
            Some("specific-crate")
        );
    }

    #[test]
    fn maps_readiness_method_variants() {
        for method in [ReadinessMethod::Api, ReadinessMethod::Index, ReadinessMethod::Both] {
            let mut opts = sample_runtime_options();
            opts.readiness.method = method;
            assert_eq!(into_runtime_options(opts).readiness.method, method);
        }
    }

    #[test]
    fn maps_readiness_jitter_factor() {
        let mut opts = sample_runtime_options();
        opts.readiness.jitter_factor = 0.0;
        let converted = into_runtime_options(opts);
        assert!((converted.readiness.jitter_factor - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn maps_readiness_prefer_index() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.readiness.prefer_index = val;
            assert_eq!(into_runtime_options(opts).readiness.prefer_index, val);
        }
    }

    #[test]
    fn maps_parallel_max_concurrent() {
        let mut opts = sample_runtime_options();
        opts.parallel.max_concurrent = 1;
        assert_eq!(into_runtime_options(opts).parallel.max_concurrent, 1);
    }

    #[test]
    fn maps_parallel_enabled() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.parallel.enabled = val;
            assert_eq!(into_runtime_options(opts).parallel.enabled, val);
        }
    }

    #[test]
    fn maps_webhook_url() {
        let mut opts = sample_runtime_options();
        opts.webhook.url = "https://hooks.example.com/notify".to_string();
        assert_eq!(
            into_runtime_options(opts).webhook.url,
            "https://hooks.example.com/notify"
        );
    }

    #[test]
    fn maps_webhook_timeout() {
        let mut opts = sample_runtime_options();
        opts.webhook.timeout_secs = 99;
        assert_eq!(into_runtime_options(opts).webhook.timeout_secs, 99);
    }

    #[test]
    fn maps_encryption_enabled() {
        for val in [true, false] {
            let mut opts = sample_runtime_options();
            opts.encryption.enabled = val;
            assert_eq!(into_runtime_options(opts).encryption.enabled, val);
        }
    }

    // ── Special characters in URL/paths ────────────────────────────────
    #[test]
    fn special_chars_in_webhook_url() {
        let mut opts = sample_runtime_options();
        opts.webhook.url =
            "https://hooks.example.com/path?key=val&foo=bar#fragment%20encoded".to_string();
        assert_eq!(
            into_runtime_options(opts).webhook.url,
            "https://hooks.example.com/path?key=val&foo=bar#fragment%20encoded"
        );
    }

    #[test]
    fn unicode_in_state_dir() {
        let mut opts = sample_runtime_options();
        opts.state_dir = PathBuf::from("/tmp/工作目录/shipper-状态");
        assert_eq!(
            into_runtime_options(opts).state_dir,
            PathBuf::from("/tmp/工作目录/shipper-状态")
        );
    }

    #[test]
    fn special_chars_in_registry_fields() {
        let mut opts = sample_runtime_options();
        opts.registries = vec![Registry {
            name: "my-org/private-reg".to_string(),
            api_base: "https://registry.example.com:8443/api/v1?token=abc&scope=all".to_string(),
            index_base: Some("https://index.example.com/path with spaces/".to_string()),
        }];
        let converted = into_runtime_options(opts);
        assert_eq!(converted.registries[0].name, "my-org/private-reg");
        assert_eq!(
            converted.registries[0].api_base,
            "https://registry.example.com:8443/api/v1?token=abc&scope=all"
        );
        assert_eq!(
            converted.registries[0].index_base.as_deref(),
            Some("https://index.example.com/path with spaces/")
        );
    }

    #[test]
    fn special_chars_in_resume_from() {
        let mut opts = sample_runtime_options();
        opts.resume_from = Some("my-crate_v2.0.0-rc.1".to_string());
        assert_eq!(
            into_runtime_options(opts).resume_from.as_deref(),
            Some("my-crate_v2.0.0-rc.1")
        );
    }

    #[test]
    fn special_chars_in_encryption_env_var() {
        let mut opts = sample_runtime_options();
        opts.encryption.env_var = Some("MY_APP__ENCRYPT_KEY_2".to_string());
        assert_eq!(
            into_runtime_options(opts).encryption.env_var.as_deref(),
            Some("MY_APP__ENCRYPT_KEY_2")
        );
    }

    #[test]
    fn empty_string_webhook_url() {
        let mut opts = sample_runtime_options();
        opts.webhook.url = String::new();
        assert_eq!(into_runtime_options(opts).webhook.url, "");
    }

    #[test]
    fn special_chars_in_readiness_index_path() {
        let mut opts = sample_runtime_options();
        opts.readiness.index_path = Some(PathBuf::from("C:\\Users\\build agent\\index (2)"));
        assert_eq!(
            into_runtime_options(opts).readiness.index_path,
            Some(PathBuf::from("C:\\Users\\build agent\\index (2)"))
        );
    }

    // ── Multiple registries edge cases ─────────────────────────────────
    #[test]
    fn single_registry() {
        let mut opts = sample_runtime_options();
        opts.registries = vec![Registry {
            name: "only".to_string(),
            api_base: "https://only.example.com".to_string(),
            index_base: None,
        }];
        let converted = into_runtime_options(opts);
        assert_eq!(converted.registries.len(), 1);
        assert_eq!(converted.registries[0].name, "only");
    }

    #[test]
    fn many_registries() {
        let mut opts = sample_runtime_options();
        opts.registries = (0..20)
            .map(|i| Registry {
                name: format!("reg-{i}"),
                api_base: format!("https://reg{i}.example.com"),
                index_base: if i % 2 == 0 {
                    Some(format!("https://index{i}.example.com"))
                } else {
                    None
                },
            })
            .collect();
        let converted = into_runtime_options(opts);
        assert_eq!(converted.registries.len(), 20);
        assert!(converted.registries[0].index_base.is_some());
        assert!(converted.registries[1].index_base.is_none());
    }

    // ── Boundary values for numeric fields ─────────────────────────────
    #[test]
    fn max_attempts_one() {
        let mut opts = sample_runtime_options();
        opts.max_attempts = 1;
        assert_eq!(into_runtime_options(opts).max_attempts, 1);
    }

    #[test]
    fn max_attempts_u32_max() {
        let mut opts = sample_runtime_options();
        opts.max_attempts = u32::MAX;
        assert_eq!(into_runtime_options(opts).max_attempts, u32::MAX);
    }

    #[test]
    fn output_lines_max() {
        let mut opts = sample_runtime_options();
        opts.output_lines = usize::MAX;
        assert_eq!(into_runtime_options(opts).output_lines, usize::MAX);
    }

    #[test]
    fn parallel_max_concurrent_zero() {
        let mut opts = sample_runtime_options();
        opts.parallel.max_concurrent = 0;
        assert_eq!(into_runtime_options(opts).parallel.max_concurrent, 0);
    }

    #[test]
    fn retry_jitter_zero() {
        let mut opts = sample_runtime_options();
        opts.retry_jitter = 0.0;
        let converted = into_runtime_options(opts);
        assert!((converted.retry_jitter).abs() < f64::EPSILON);
    }

    #[test]
    fn retry_jitter_one() {
        let mut opts = sample_runtime_options();
        opts.retry_jitter = 1.0;
        let converted = into_runtime_options(opts);
        assert!((converted.retry_jitter - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn webhook_timeout_zero() {
        let mut opts = sample_runtime_options();
        opts.webhook.timeout_secs = 0;
        assert_eq!(into_runtime_options(opts).webhook.timeout_secs, 0);
    }

    mod snapshot_tests {
        use super::*;
        use insta::assert_debug_snapshot;

        fn default_config_runtime() -> RuntimeOptions {
            RuntimeOptions {
                allow_dirty: false,
                skip_ownership_check: false,
                strict_ownership: false,
                no_verify: false,
                max_attempts: 3,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(300),
                retry_strategy: shipper_retry::RetryStrategyType::Exponential,
                retry_jitter: 0.25,
                retry_per_error: shipper_retry::PerErrorConfig::default(),
                verify_timeout: Duration::from_secs(60),
                verify_poll_interval: Duration::from_secs(5),
                state_dir: PathBuf::from(".shipper"),
                force_resume: false,
                policy: PublishPolicy::Safe,
                verify_mode: VerifyMode::Workspace,
                readiness: ReadinessConfig {
                    enabled: false,
                    method: ReadinessMethod::Api,
                    initial_delay: Duration::from_millis(100),
                    max_delay: Duration::from_secs(60),
                    max_total_wait: Duration::from_secs(300),
                    poll_interval: Duration::from_secs(5),
                    jitter_factor: 0.25,
                    prefer_index: false,
                    index_path: None,
                },
                output_lines: 20,
                force: false,
                lock_timeout: Duration::from_secs(30),
                parallel: ParallelConfig {
                    enabled: false,
                    max_concurrent: 4,
                    per_package_timeout: Duration::from_secs(120),
                },
                webhook: WebhookConfig {
                    url: String::new(),
                    secret: None,
                    ..WebhookConfig::default()
                },
                encryption: EncryptionConfig {
                    enabled: false,
                    passphrase: None,
                    ..EncryptionConfig::default()
                },
                registries: vec![],
                resume_from: None,
            }
        }

        #[test]
        fn snapshot_default_conversion() {
            let cfg = default_config_runtime();
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_all_flags_enabled() {
            let mut cfg = default_config_runtime();
            cfg.allow_dirty = true;
            cfg.skip_ownership_check = true;
            cfg.strict_ownership = true;
            cfg.no_verify = true;
            cfg.force_resume = true;
            cfg.force = true;
            cfg.parallel.enabled = true;
            cfg.readiness.enabled = true;
            cfg.encryption.enabled = true;
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_with_registries() {
            let mut cfg = default_config_runtime();
            cfg.registries = vec![
                Registry {
                    name: "crates-io".to_string(),
                    api_base: "https://crates.io".to_string(),
                    index_base: Some("https://index.crates.io".to_string()),
                },
                Registry {
                    name: "private".to_string(),
                    api_base: "https://my-registry.example.com".to_string(),
                    index_base: None,
                },
            ];
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_fast_policy_no_verify() {
            let mut cfg = default_config_runtime();
            cfg.policy = PublishPolicy::Fast;
            cfg.verify_mode = VerifyMode::None;
            cfg.no_verify = true;
            cfg.max_attempts = 1;
            cfg.base_delay = Duration::ZERO;
            cfg.max_delay = Duration::ZERO;
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_full_readiness_config() {
            let mut cfg = default_config_runtime();
            cfg.readiness = ReadinessConfig {
                enabled: true,
                method: ReadinessMethod::Both,
                initial_delay: Duration::from_millis(500),
                max_delay: Duration::from_secs(120),
                max_total_wait: Duration::from_secs(600),
                poll_interval: Duration::from_secs(10),
                jitter_factor: 0.5,
                prefer_index: true,
                index_path: Some(PathBuf::from("/custom/index")),
            };
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_parallel_heavy() {
            let mut cfg = default_config_runtime();
            cfg.parallel = ParallelConfig {
                enabled: true,
                max_concurrent: 16,
                per_package_timeout: Duration::from_secs(3600),
            };
            cfg.lock_timeout = Duration::from_secs(7200);
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_webhook_with_secret() {
            let mut cfg = default_config_runtime();
            cfg.webhook = WebhookConfig {
                url: "https://hooks.slack.com/services/T00/B00/xxxx".to_string(),
                secret: Some("hmac-secret-key".to_string()),
                timeout_secs: 5,
                ..WebhookConfig::default()
            };
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_encryption_with_env_var() {
            let mut cfg = default_config_runtime();
            cfg.encryption = EncryptionConfig {
                enabled: true,
                passphrase: None,
                env_var: Some("CI_ENCRYPT_KEY".to_string()),
            };
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_linear_retry_strategy() {
            let mut cfg = default_config_runtime();
            cfg.retry_strategy = shipper_retry::RetryStrategyType::Linear;
            cfg.retry_jitter = 0.0;
            cfg.max_attempts = 10;
            cfg.base_delay = Duration::from_millis(100);
            cfg.max_delay = Duration::from_secs(10);
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }

        #[test]
        fn snapshot_resume_from_set() {
            let mut cfg = default_config_runtime();
            cfg.resume_from = Some("my-sub-crate".to_string());
            cfg.force_resume = true;
            let converted = into_runtime_options(cfg);
            assert_debug_snapshot!(converted);
        }
    }
}
