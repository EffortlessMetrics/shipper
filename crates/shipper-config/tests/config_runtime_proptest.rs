//! Property-based tests for config resolution, CLI override precedence,
//! default validity, and ShipperConfig round-trip serialization.

use std::path::PathBuf;
use std::time::Duration;

use proptest::prelude::*;

use shipper_config::runtime::into_runtime_options;
use shipper_config::{
    CliOverrides, EncryptionConfigInner, FlagsConfig, LockConfig, MultiRegistryConfig,
    OutputConfig, PolicyConfig, ReadinessConfig, ReadinessMethod, RegistryConfig, RetryConfig,
    ShipperConfig, VerifyConfig, WebhookConfig,
};
use shipper_retry::{RetryPolicy, RetryStrategyType};
use shipper_types::{ParallelConfig, PublishPolicy, VerifyMode};

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

fn arb_publish_policy() -> impl Strategy<Value = PublishPolicy> {
    prop_oneof![
        Just(PublishPolicy::Safe),
        Just(PublishPolicy::Balanced),
        Just(PublishPolicy::Fast),
    ]
}

fn arb_verify_mode() -> impl Strategy<Value = VerifyMode> {
    prop_oneof![
        Just(VerifyMode::Workspace),
        Just(VerifyMode::Package),
        Just(VerifyMode::None),
    ]
}

fn arb_readiness_method() -> impl Strategy<Value = ReadinessMethod> {
    prop_oneof![
        Just(ReadinessMethod::Api),
        Just(ReadinessMethod::Index),
        Just(ReadinessMethod::Both),
    ]
}

fn arb_retry_policy() -> impl Strategy<Value = RetryPolicy> {
    prop_oneof![
        Just(RetryPolicy::Default),
        Just(RetryPolicy::Aggressive),
        Just(RetryPolicy::Conservative),
        Just(RetryPolicy::Custom),
    ]
}

fn arb_retry_strategy() -> impl Strategy<Value = RetryStrategyType> {
    prop_oneof![
        Just(RetryStrategyType::Immediate),
        Just(RetryStrategyType::Exponential),
        Just(RetryStrategyType::Linear),
        Just(RetryStrategyType::Constant),
    ]
}

/// Non-zero duration in a sensible range.
fn arb_nonzero_duration_ms(lo: u64, hi: u64) -> impl Strategy<Value = Duration> {
    (lo..=hi).prop_map(Duration::from_millis)
}

/// Alphanumeric string of bounded length (avoids edge-cases with empty names).
fn arb_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,15}".prop_map(String::from)
}

fn arb_url() -> impl Strategy<Value = String> {
    arb_name().prop_map(|n| format!("https://{n}.example"))
}

fn arb_readiness_config() -> impl Strategy<Value = ReadinessConfig> {
    (
        any::<bool>(),
        arb_readiness_method(),
        arb_nonzero_duration_ms(1, 5_000),
        arb_nonzero_duration_ms(1_000, 120_000),
        arb_nonzero_duration_ms(1_000, 600_000),
        arb_nonzero_duration_ms(100, 30_000),
        0.0..=1.0_f64,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                enabled,
                method,
                initial_delay,
                max_delay,
                max_total_wait,
                poll_interval,
                jitter_factor,
                prefer_index,
                has_index_path,
            )| {
                ReadinessConfig {
                    enabled,
                    method,
                    initial_delay,
                    max_delay,
                    max_total_wait,
                    poll_interval,
                    jitter_factor,
                    index_path: if has_index_path {
                        Some(PathBuf::from("test-index"))
                    } else {
                        None
                    },
                    prefer_index,
                }
            },
        )
}

fn arb_parallel_config() -> impl Strategy<Value = ParallelConfig> {
    (
        any::<bool>(),
        1usize..32,
        arb_nonzero_duration_ms(1_000, 600_000),
    )
        .prop_map(
            |(enabled, max_concurrent, per_package_timeout)| ParallelConfig {
                enabled,
                max_concurrent,
                per_package_timeout,
            },
        )
}

fn arb_flags_config() -> impl Strategy<Value = FlagsConfig> {
    (any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(allow_dirty, skip_ownership_check, strict_ownership)| FlagsConfig {
            allow_dirty,
            skip_ownership_check,
            strict_ownership,
        },
    )
}

fn arb_webhook_config() -> impl Strategy<Value = WebhookConfig> {
    (arb_url(), proptest::option::of(arb_name()), 1u64..120).prop_map(
        |(url, secret, timeout_secs)| WebhookConfig {
            url,
            webhook_type: Default::default(),
            secret,
            timeout_secs,
        },
    )
}

fn arb_encryption_config() -> impl Strategy<Value = EncryptionConfigInner> {
    (
        any::<bool>(),
        proptest::option::of(arb_name()),
        proptest::option::of(arb_name()),
    )
        .prop_map(|(enabled, passphrase, env_key)| EncryptionConfigInner {
            enabled,
            passphrase,
            env_key,
        })
}

fn arb_retry_config() -> impl Strategy<Value = RetryConfig> {
    (
        arb_retry_policy(),
        1u32..20,
        arb_nonzero_duration_ms(1, 10_000),
        arb_nonzero_duration_ms(1_000, 300_000),
        arb_retry_strategy(),
        0.0..=1.0_f64,
    )
        .prop_map(
            |(policy, max_attempts, base_delay, max_delay, strategy, jitter)| RetryConfig {
                policy,
                max_attempts,
                // Ensure base_delay <= max_delay for Custom policy validation
                base_delay: Duration::from_millis(base_delay.as_millis() as u64),
                max_delay: Duration::from_millis(
                    max_delay.as_millis().max(base_delay.as_millis() + 1) as u64,
                ),
                strategy,
                jitter,
                per_error: Default::default(),
            },
        )
}

fn arb_registry_configs() -> impl Strategy<Value = Vec<RegistryConfig>> {
    proptest::collection::vec(
        (arb_name(), arb_url()).prop_map(|(name, api_base)| RegistryConfig {
            name,
            api_base,
            index_base: None,
            token: None,
            default: false,
        }),
        0..4,
    )
}

fn arb_shipper_config() -> impl Strategy<Value = ShipperConfig> {
    (
        arb_publish_policy(),
        arb_verify_mode(),
        arb_readiness_config(),
        1usize..2000,
        arb_nonzero_duration_ms(1_000, 7_200_000),
        arb_retry_config(),
        arb_flags_config(),
        arb_parallel_config(),
        arb_webhook_config(),
        arb_encryption_config(),
        arb_registry_configs(),
        any::<bool>(),
    )
        .prop_map(
            |(
                policy,
                verify_mode,
                readiness,
                output_lines,
                lock_timeout,
                retry,
                flags,
                parallel,
                webhook,
                encryption,
                registries,
                has_state_dir,
            )| {
                ShipperConfig {
                    schema_version: "shipper.config.v1".to_string(),
                    policy: PolicyConfig { mode: policy },
                    verify: VerifyConfig { mode: verify_mode },
                    readiness,
                    output: OutputConfig {
                        lines: output_lines,
                    },
                    lock: LockConfig {
                        timeout: lock_timeout,
                    },
                    retry,
                    flags,
                    parallel,
                    state_dir: if has_state_dir {
                        Some(PathBuf::from("custom-state"))
                    } else {
                        None
                    },
                    registry: None,
                    registries: MultiRegistryConfig {
                        registries,
                        default_registries: vec![],
                    },
                    webhook,
                    encryption,
                    storage: Default::default(),
                    rehearsal: Default::default(),
                }
            },
        )
}

fn arb_cli_overrides() -> impl Strategy<Value = CliOverrides> {
    (
        proptest::option::of(arb_publish_policy()),
        proptest::option::of(arb_verify_mode()),
        proptest::option::of(1u32..20),
        proptest::option::of(arb_nonzero_duration_ms(1, 10_000)),
        proptest::option::of(arb_nonzero_duration_ms(1_000, 300_000)),
        proptest::option::of(arb_retry_strategy()),
        proptest::option::of(0.0..=1.0_f64),
        proptest::option::of(arb_nonzero_duration_ms(1_000, 600_000)),
        proptest::option::of(arb_nonzero_duration_ms(100, 30_000)),
        proptest::option::of(1usize..2000),
        (
            proptest::option::of(arb_nonzero_duration_ms(1_000, 7_200_000)),
            proptest::option::of(Just(PathBuf::from("cli-state"))),
            proptest::option::of(arb_readiness_method()),
            proptest::option::of(arb_nonzero_duration_ms(1_000, 600_000)),
            proptest::option::of(arb_nonzero_duration_ms(100, 30_000)),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        ),
        (
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            proptest::option::of(1usize..32),
            proptest::option::of(arb_nonzero_duration_ms(1_000, 600_000)),
            proptest::option::of(arb_url()),
            proptest::option::of(arb_name()),
            any::<bool>(),
            proptest::option::of(arb_name()),
        ),
    )
        .prop_map(
            |(
                policy,
                verify_mode,
                max_attempts,
                base_delay,
                max_delay,
                retry_strategy,
                retry_jitter,
                verify_timeout,
                verify_poll_interval,
                output_lines,
                (
                    lock_timeout,
                    state_dir,
                    readiness_method,
                    readiness_timeout,
                    readiness_poll,
                    allow_dirty,
                    skip_ownership_check,
                    strict_ownership,
                    no_verify,
                    no_readiness,
                ),
                (
                    force,
                    force_resume,
                    parallel_enabled,
                    max_concurrent,
                    per_package_timeout,
                    webhook_url,
                    webhook_secret,
                    encrypt,
                    encrypt_passphrase,
                ),
            )| {
                CliOverrides {
                    policy,
                    verify_mode,
                    max_attempts,
                    base_delay,
                    max_delay,
                    retry_strategy,
                    retry_jitter,
                    verify_timeout,
                    verify_poll_interval,
                    output_lines,
                    lock_timeout,
                    state_dir,
                    readiness_method,
                    readiness_timeout,
                    readiness_poll,
                    allow_dirty,
                    skip_ownership_check,
                    strict_ownership,
                    no_verify,
                    no_readiness,
                    force,
                    force_resume,
                    parallel_enabled,
                    max_concurrent,
                    per_package_timeout,
                    webhook_url,
                    webhook_secret,
                    encrypt,
                    encrypt_passphrase,
                    registries: None,
                    all_registries: false,
                    resume_from: None,
                    rehearsal_registry: None,
                    skip_rehearsal: false,
                    rehearsal_smoke_install: None,
                }
            },
        )
}

// ---------------------------------------------------------------------------
// 1. Config resolution from file + CLI flags always produces valid RuntimeConfig
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn config_plus_cli_always_produces_valid_runtime(
        cfg in arb_shipper_config(),
        cli in arb_cli_overrides(),
    ) {
        let merged = cfg.build_runtime_options(cli);
        let rt = into_runtime_options(merged);

        // Basic validity invariants
        prop_assert!(rt.max_attempts >= 1);
        prop_assert!(!rt.base_delay.is_zero() || rt.retry_strategy == RetryStrategyType::Immediate);
        prop_assert!(rt.output_lines >= 1);
        prop_assert!(!rt.lock_timeout.is_zero());
        prop_assert!(rt.parallel.max_concurrent >= 1);
        prop_assert!(!rt.parallel.per_package_timeout.is_zero());
        prop_assert!(rt.retry_jitter >= 0.0 && rt.retry_jitter <= 1.0);
        prop_assert!(rt.readiness.jitter_factor >= 0.0 && rt.readiness.jitter_factor <= 1.0);
    }
}

// ---------------------------------------------------------------------------
// 2. CLI flag values always override file values
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn cli_policy_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_policy in arb_publish_policy(),
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            policy: Some(cli_policy),
            ..Default::default()
        }));
        prop_assert_eq!(rt.policy, cli_policy);
    }

    #[test]
    fn cli_verify_mode_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_vm in arb_verify_mode(),
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            verify_mode: Some(cli_vm),
            ..Default::default()
        }));
        prop_assert_eq!(rt.verify_mode, cli_vm);
    }

    #[test]
    fn cli_output_lines_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_lines in 1usize..5000,
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            output_lines: Some(cli_lines),
            ..Default::default()
        }));
        prop_assert_eq!(rt.output_lines, cli_lines);
    }

    #[test]
    fn cli_lock_timeout_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_timeout_ms in 1u64..7_200_000,
    ) {
        let expected = Duration::from_millis(cli_timeout_ms);
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            lock_timeout: Some(expected),
            ..Default::default()
        }));
        prop_assert_eq!(rt.lock_timeout, expected);
    }

    #[test]
    fn cli_max_attempts_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_attempts in 1u32..100,
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            max_attempts: Some(cli_attempts),
            ..Default::default()
        }));
        prop_assert_eq!(rt.max_attempts, cli_attempts);
    }

    #[test]
    fn cli_readiness_method_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_method in arb_readiness_method(),
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            readiness_method: Some(cli_method),
            ..Default::default()
        }));
        prop_assert_eq!(rt.readiness.method, cli_method);
    }

    #[test]
    fn cli_max_concurrent_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_concurrent in 1usize..64,
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            max_concurrent: Some(cli_concurrent),
            ..Default::default()
        }));
        prop_assert_eq!(rt.parallel.max_concurrent, cli_concurrent);
    }

    #[test]
    fn cli_state_dir_always_overrides_config(
        cfg in arb_shipper_config(),
    ) {
        let cli_dir = PathBuf::from("cli-override-dir");
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            state_dir: Some(cli_dir.clone()),
            ..Default::default()
        }));
        prop_assert_eq!(rt.state_dir, cli_dir);
    }

    #[test]
    fn cli_webhook_url_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_url in arb_url(),
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            webhook_url: Some(cli_url.clone()),
            ..Default::default()
        }));
        prop_assert_eq!(rt.webhook.url, cli_url);
    }

    #[test]
    fn cli_webhook_secret_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_secret in arb_name(),
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            webhook_secret: Some(cli_secret.clone()),
            ..Default::default()
        }));
        prop_assert_eq!(rt.webhook.secret.as_deref(), Some(cli_secret.as_str()));
    }

    #[test]
    fn cli_retry_strategy_always_overrides_config(
        cfg in arb_shipper_config(),
        cli_strategy in arb_retry_strategy(),
    ) {
        let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
            retry_strategy: Some(cli_strategy),
            ..Default::default()
        }));
        prop_assert_eq!(rt.retry_strategy, cli_strategy);
    }
}

// ---------------------------------------------------------------------------
// 3. Default values are always valid
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn default_config_with_random_cli_always_valid(
        cli in arb_cli_overrides(),
    ) {
        let rt = into_runtime_options(ShipperConfig::default().build_runtime_options(cli));

        prop_assert!(rt.max_attempts >= 1);
        prop_assert!(rt.output_lines >= 1);
        prop_assert!(!rt.lock_timeout.is_zero());
        prop_assert!(rt.parallel.max_concurrent >= 1);
        prop_assert!(!rt.parallel.per_package_timeout.is_zero());
        prop_assert!(rt.retry_jitter >= 0.0 && rt.retry_jitter <= 1.0);
        prop_assert!(rt.readiness.jitter_factor >= 0.0 && rt.readiness.jitter_factor <= 1.0);
    }

    #[test]
    fn default_config_with_empty_cli_yields_stable_defaults(
        _seed in 0u32..100,
    ) {
        let rt = into_runtime_options(
            ShipperConfig::default().build_runtime_options(CliOverrides::default()),
        );

        // Defaults must be deterministic regardless of seed.
        prop_assert_eq!(rt.policy, PublishPolicy::Safe);
        prop_assert_eq!(rt.verify_mode, VerifyMode::Workspace);
        prop_assert_eq!(rt.output_lines, 50);
        prop_assert_eq!(rt.lock_timeout, Duration::from_secs(3_600));
        prop_assert_eq!(rt.max_attempts, 6);
        prop_assert!(!rt.allow_dirty);
        prop_assert!(!rt.skip_ownership_check);
        prop_assert!(!rt.strict_ownership);
        prop_assert!(!rt.no_verify);
        prop_assert!(!rt.force);
        prop_assert!(!rt.force_resume);
        prop_assert!(rt.readiness.enabled);
        prop_assert!(!rt.parallel.enabled);
        prop_assert!(rt.webhook.url.is_empty());
        prop_assert!(rt.webhook.secret.is_none());
        prop_assert!(rt.registries.is_empty());
        prop_assert!(rt.resume_from.is_none());
    }
}

// ---------------------------------------------------------------------------
// 4. Round-trip: random ShipperConfig → serialize TOML → deserialize → compare
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn shipper_config_toml_roundtrip(cfg in arb_shipper_config()) {
        let toml_str = toml::to_string(&cfg).expect("serialize to TOML");
        let deserialized: ShipperConfig =
            toml::from_str(&toml_str).expect("deserialize from TOML");

        // Double round-trip: serialize the deserialized config again and compare TOML.
        let toml_str2 = toml::to_string(&deserialized).expect("re-serialize to TOML");
        prop_assert_eq!(toml_str, toml_str2);

        // Compare key fields (ShipperConfig lacks PartialEq).
        prop_assert_eq!(&deserialized.schema_version, &cfg.schema_version);
        prop_assert_eq!(deserialized.policy.mode, cfg.policy.mode);
        prop_assert_eq!(deserialized.verify.mode, cfg.verify.mode);
        prop_assert_eq!(deserialized.readiness.enabled, cfg.readiness.enabled);
        prop_assert_eq!(deserialized.readiness.method, cfg.readiness.method);
        prop_assert_eq!(
            deserialized.readiness.initial_delay,
            cfg.readiness.initial_delay
        );
        prop_assert_eq!(deserialized.readiness.max_delay, cfg.readiness.max_delay);
        prop_assert_eq!(
            deserialized.readiness.max_total_wait,
            cfg.readiness.max_total_wait
        );
        prop_assert_eq!(
            deserialized.readiness.poll_interval,
            cfg.readiness.poll_interval
        );
        prop_assert!(
            (deserialized.readiness.jitter_factor - cfg.readiness.jitter_factor).abs()
                < f64::EPSILON
        );
        prop_assert_eq!(deserialized.readiness.prefer_index, cfg.readiness.prefer_index);
        prop_assert_eq!(&deserialized.readiness.index_path, &cfg.readiness.index_path);
        prop_assert_eq!(deserialized.output.lines, cfg.output.lines);
        prop_assert_eq!(deserialized.lock.timeout, cfg.lock.timeout);
        prop_assert_eq!(deserialized.retry.policy, cfg.retry.policy);
        prop_assert_eq!(deserialized.retry.max_attempts, cfg.retry.max_attempts);
        prop_assert_eq!(deserialized.retry.base_delay, cfg.retry.base_delay);
        prop_assert_eq!(deserialized.retry.max_delay, cfg.retry.max_delay);
        prop_assert_eq!(deserialized.retry.strategy, cfg.retry.strategy);
        prop_assert!((deserialized.retry.jitter - cfg.retry.jitter).abs() < f64::EPSILON);
        prop_assert_eq!(deserialized.flags.allow_dirty, cfg.flags.allow_dirty);
        prop_assert_eq!(
            deserialized.flags.skip_ownership_check,
            cfg.flags.skip_ownership_check
        );
        prop_assert_eq!(
            deserialized.flags.strict_ownership,
            cfg.flags.strict_ownership
        );
        prop_assert_eq!(deserialized.parallel.enabled, cfg.parallel.enabled);
        prop_assert_eq!(
            deserialized.parallel.max_concurrent,
            cfg.parallel.max_concurrent
        );
        prop_assert_eq!(
            deserialized.parallel.per_package_timeout,
            cfg.parallel.per_package_timeout
        );
        prop_assert_eq!(&deserialized.webhook.url, &cfg.webhook.url);
        prop_assert_eq!(&deserialized.webhook.secret, &cfg.webhook.secret);
        prop_assert_eq!(deserialized.webhook.timeout_secs, cfg.webhook.timeout_secs);
        prop_assert_eq!(deserialized.encryption.enabled, cfg.encryption.enabled);
        prop_assert_eq!(&deserialized.encryption.passphrase, &cfg.encryption.passphrase);
        prop_assert_eq!(&deserialized.encryption.env_key, &cfg.encryption.env_key);
        prop_assert_eq!(&deserialized.state_dir, &cfg.state_dir);
        prop_assert_eq!(
            deserialized.registries.registries.len(),
            cfg.registries.registries.len()
        );
        for (d, o) in deserialized
            .registries
            .registries
            .iter()
            .zip(cfg.registries.registries.iter())
        {
            prop_assert_eq!(&d.name, &o.name);
            prop_assert_eq!(&d.api_base, &o.api_base);
        }
    }
}
