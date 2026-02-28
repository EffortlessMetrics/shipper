use std::path::PathBuf;
use std::time::Duration;

use shipper_config::{
    CliOverrides, MultiRegistryConfig, PolicyConfig, ReadinessConfig, ReadinessMethod,
    RegistryConfig, ShipperConfig,
};
use shipper_config_runtime::into_runtime_options;
use shipper_retry::RetryStrategyType;
use shipper_types::{ParallelConfig, PublishPolicy, VerifyMode};

/// Helper: default ShipperConfig for tests that need a clean baseline.
fn default_config() -> ShipperConfig {
    ShipperConfig::default()
}

/// Helper: a ShipperConfig with non-default values in every section so we can
/// verify that config-file values survive when CLI overrides are absent.
fn custom_config() -> ShipperConfig {
    ShipperConfig {
        schema_version: "shipper.config.v1".to_string(),
        policy: PolicyConfig {
            mode: PublishPolicy::Fast,
        },
        verify: shipper_config::VerifyConfig {
            mode: VerifyMode::None,
        },
        readiness: ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            max_total_wait: Duration::from_secs(200),
            poll_interval: Duration::from_secs(7),
            jitter_factor: 0.15,
            index_path: Some(PathBuf::from("custom-index")),
            prefer_index: true,
        },
        output: shipper_config::OutputConfig { lines: 300 },
        lock: shipper_config::LockConfig {
            timeout: Duration::from_secs(1_800),
        },
        retry: shipper_config::RetryConfig {
            policy: shipper_retry::RetryPolicy::Aggressive,
            ..shipper_config::RetryConfig::default()
        },
        flags: shipper_config::FlagsConfig {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: true,
        },
        parallel: ParallelConfig {
            enabled: true,
            max_concurrent: 8,
            per_package_timeout: Duration::from_secs(60),
        },
        state_dir: Some(PathBuf::from("custom-state")),
        registry: None,
        registries: MultiRegistryConfig::default(),
        webhook: shipper_config::WebhookConfig {
            url: "https://hooks.custom.local".to_string(),
            webhook_type: Default::default(),
            secret: Some("file-secret".to_string()),
            timeout_secs: 45,
        },
        encryption: shipper_config::EncryptionConfigInner {
            enabled: true,
            passphrase: Some("file-passphrase".to_string()),
            env_key: Some("CUSTOM_KEY".to_string()),
        },
        storage: shipper_config::StorageConfigInner::default(),
    }
}

#[test]
fn converts_config_to_runtime_contract_with_registry_overrides() {
    let source = ShipperConfig {
        schema_version: "shipper.config.v1".to_string(),
        policy: PolicyConfig {
            mode: PublishPolicy::Safe,
        },
        verify: shipper_config::VerifyConfig {
            mode: VerifyMode::Workspace,
        },
        readiness: ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(45),
            max_total_wait: Duration::from_secs(360),
            poll_interval: Duration::from_secs(2),
            jitter_factor: 0.3,
            index_path: Some(PathBuf::from("/tmp/index")),
            prefer_index: true,
        },
        output: shipper_config::OutputConfig { lines: 101 },
        lock: shipper_config::LockConfig {
            timeout: Duration::from_secs(900),
        },
        retry: shipper_config::RetryConfig::default(),
        flags: shipper_config::FlagsConfig {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: false,
        },
        parallel: ParallelConfig {
            enabled: true,
            max_concurrent: 9,
            per_package_timeout: Duration::from_secs(12),
        },
        state_dir: Some(PathBuf::from(".shipper")),
        registry: None,
        registries: shipper_config::MultiRegistryConfig::default(),
        webhook: shipper_config::WebhookConfig {
            url: "https://hooks.example.local".to_string(),
            webhook_type: Default::default(),
            secret: Some("abc".to_string()),
            timeout_secs: 20,
        },
        encryption: shipper_config::EncryptionConfigInner::default(),
        storage: shipper_config::StorageConfigInner::default(),
    };

    let merged = source.build_runtime_options(CliOverrides {
        max_attempts: Some(11),
        output_lines: Some(256),
        policy: Some(PublishPolicy::Safe),
        verify_mode: Some(VerifyMode::Package),
        readiness_timeout: Some(Duration::from_secs(2)),
        readiness_poll: Some(Duration::from_secs(3)),
        readiness_method: Some(ReadinessMethod::Index),
        webhook_url: Some("https://override.example/webhook".to_string()),
        webhook_secret: Some("secret2".to_string()),
        max_concurrent: Some(3),
        ..Default::default()
    });

    let runtime = into_runtime_options(merged);

    assert_eq!(runtime.output_lines, 256);
    assert_eq!(runtime.max_attempts, 11);
    assert_eq!(runtime.policy, PublishPolicy::Safe);
    assert_eq!(runtime.verify_mode, VerifyMode::Package);
    assert_eq!(runtime.readiness.method, ReadinessMethod::Index);
    assert_eq!(runtime.parallel.max_concurrent, 3);
    assert_eq!(runtime.webhook.url, "https://override.example/webhook");
    assert_eq!(runtime.webhook.secret.as_deref(), Some("secret2"));
    assert_eq!(runtime.webhook.timeout_secs, 20);
    assert_eq!(runtime.lock_timeout, Duration::from_secs(900));
}

// ---------------------------------------------------------------------------
// Precedence: empty CLI overrides → config file values used
// ---------------------------------------------------------------------------

#[test]
fn empty_cli_overrides_yields_all_config_file_values() {
    let cfg = custom_config();
    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides::default()));

    assert_eq!(rt.policy, PublishPolicy::Fast);
    assert_eq!(rt.verify_mode, VerifyMode::None);
    assert_eq!(rt.output_lines, 300);
    assert_eq!(rt.lock_timeout, Duration::from_secs(1_800));
    assert_eq!(rt.readiness.method, ReadinessMethod::Index);
    assert_eq!(rt.readiness.poll_interval, Duration::from_secs(7));
    assert_eq!(rt.readiness.max_total_wait, Duration::from_secs(200));
    assert!(rt.readiness.prefer_index);
    assert_eq!(rt.parallel.max_concurrent, 8);
    assert!(rt.parallel.enabled);
    assert_eq!(rt.state_dir, PathBuf::from("custom-state"));
    assert_eq!(rt.webhook.url, "https://hooks.custom.local");
    assert_eq!(rt.webhook.secret.as_deref(), Some("file-secret"));
    assert_eq!(rt.webhook.timeout_secs, 45);
}

#[test]
fn default_config_with_empty_overrides_yields_all_defaults() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));

    assert_eq!(rt.policy, PublishPolicy::Safe);
    assert_eq!(rt.verify_mode, VerifyMode::Workspace);
    assert_eq!(rt.output_lines, 50);
    assert_eq!(rt.lock_timeout, Duration::from_secs(3_600));
    assert_eq!(rt.max_attempts, 6);
    assert_eq!(rt.base_delay, Duration::from_secs(2));
    assert_eq!(rt.max_delay, Duration::from_secs(120));
    assert_eq!(rt.state_dir, PathBuf::from(".shipper"));
    assert!(!rt.allow_dirty);
    assert!(!rt.skip_ownership_check);
    assert!(!rt.strict_ownership);
    assert!(!rt.no_verify);
    assert!(!rt.force);
    assert!(!rt.force_resume);
    assert!(rt.readiness.enabled);
    assert!(!rt.parallel.enabled);
    assert!(rt.webhook.url.is_empty());
    assert!(rt.webhook.secret.is_none());
    assert!(rt.registries.is_empty());
    assert!(rt.resume_from.is_none());
}

// ---------------------------------------------------------------------------
// Precedence: CLI overrides beat config file
// ---------------------------------------------------------------------------

#[test]
fn cli_policy_overrides_config_file_policy() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        policy: Some(PublishPolicy::Balanced),
        ..Default::default()
    }));

    // Config had Fast, CLI says Balanced → Balanced wins.
    assert_eq!(rt.policy, PublishPolicy::Balanced);
}

#[test]
fn cli_verify_mode_overrides_config_file() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        verify_mode: Some(VerifyMode::Package),
        ..Default::default()
    }));

    assert_eq!(rt.verify_mode, VerifyMode::Package);
}

#[test]
fn cli_output_lines_overrides_config_file() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        output_lines: Some(999),
        ..Default::default()
    }));

    assert_eq!(rt.output_lines, 999);
}

#[test]
fn cli_lock_timeout_overrides_config_file() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        lock_timeout: Some(Duration::from_secs(42)),
        ..Default::default()
    }));

    assert_eq!(rt.lock_timeout, Duration::from_secs(42));
}

#[test]
fn cli_state_dir_overrides_config_file() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        state_dir: Some(PathBuf::from("cli-state")),
        ..Default::default()
    }));

    assert_eq!(rt.state_dir, PathBuf::from("cli-state"));
}

#[test]
fn state_dir_falls_back_to_default_when_neither_cli_nor_config() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));

    assert_eq!(rt.state_dir, PathBuf::from(".shipper"));
}

// ---------------------------------------------------------------------------
// Boolean flag OR-semantics (allow_dirty, skip_ownership_check, strict_ownership)
// ---------------------------------------------------------------------------

#[test]
fn boolean_flags_or_both_false() {
    let cfg = default_config(); // all flags false
    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides::default()));

    assert!(!rt.allow_dirty);
    assert!(!rt.skip_ownership_check);
    assert!(!rt.strict_ownership);
}

#[test]
fn boolean_flags_or_config_true_cli_false() {
    let cfg = custom_config(); // all flags true
    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides::default()));

    assert!(rt.allow_dirty);
    assert!(rt.skip_ownership_check);
    assert!(rt.strict_ownership);
}

#[test]
fn boolean_flags_or_cli_true_config_false() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        allow_dirty: true,
        skip_ownership_check: true,
        strict_ownership: true,
        ..Default::default()
    }));

    assert!(rt.allow_dirty);
    assert!(rt.skip_ownership_check);
    assert!(rt.strict_ownership);
}

#[test]
fn boolean_flags_or_both_true() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        allow_dirty: true,
        skip_ownership_check: true,
        strict_ownership: true,
        ..Default::default()
    }));

    assert!(rt.allow_dirty);
    assert!(rt.skip_ownership_check);
    assert!(rt.strict_ownership);
}

// ---------------------------------------------------------------------------
// CLI-only boolean flags (no_verify, force, force_resume)
// ---------------------------------------------------------------------------

#[test]
fn no_verify_comes_from_cli_only() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        no_verify: true,
        ..Default::default()
    }));
    assert!(rt.no_verify);

    let rt2 = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));
    assert!(!rt2.no_verify);
}

#[test]
fn force_and_force_resume_come_from_cli_only() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        force: true,
        force_resume: true,
        ..Default::default()
    }));
    assert!(rt.force);
    assert!(rt.force_resume);
}

// ---------------------------------------------------------------------------
// Readiness config overrides
// ---------------------------------------------------------------------------

#[test]
fn no_readiness_flag_disables_readiness() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        no_readiness: true,
        ..Default::default()
    }));

    assert!(!rt.readiness.enabled);
}

#[test]
fn readiness_method_override() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        readiness_method: Some(ReadinessMethod::Api),
        ..Default::default()
    }));

    assert_eq!(rt.readiness.method, ReadinessMethod::Api);
}

#[test]
fn readiness_timeout_and_poll_overrides() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        readiness_timeout: Some(Duration::from_secs(99)),
        readiness_poll: Some(Duration::from_secs(11)),
        ..Default::default()
    }));

    assert_eq!(rt.readiness.max_total_wait, Duration::from_secs(99));
    assert_eq!(rt.readiness.poll_interval, Duration::from_secs(11));
}

#[test]
fn readiness_preserves_config_only_fields_when_cli_overrides_others() {
    // index_path, prefer_index, initial_delay, jitter_factor come only from config
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        readiness_method: Some(ReadinessMethod::Both),
        ..Default::default()
    }));

    assert_eq!(rt.readiness.method, ReadinessMethod::Both);
    assert!(rt.readiness.prefer_index);
    assert_eq!(
        rt.readiness.index_path.as_deref(),
        Some(std::path::Path::new("custom-index"))
    );
    assert_eq!(rt.readiness.initial_delay, Duration::from_millis(500));
    assert_eq!(rt.readiness.jitter_factor, 0.15);
}

// ---------------------------------------------------------------------------
// Parallel config overrides
// ---------------------------------------------------------------------------

#[test]
fn parallel_enabled_or_semantics() {
    // Config disabled, CLI enabled → enabled
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        parallel_enabled: true,
        ..Default::default()
    }));
    assert!(rt.parallel.enabled);

    // Config enabled, CLI disabled → enabled (OR)
    let rt2 = into_runtime_options(custom_config().build_runtime_options(CliOverrides::default()));
    assert!(rt2.parallel.enabled);
}

#[test]
fn parallel_max_concurrent_override() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        max_concurrent: Some(16),
        ..Default::default()
    }));
    assert_eq!(rt.parallel.max_concurrent, 16);
}

#[test]
fn parallel_per_package_timeout_override() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        per_package_timeout: Some(Duration::from_secs(300)),
        ..Default::default()
    }));
    assert_eq!(rt.parallel.per_package_timeout, Duration::from_secs(300));
}

// ---------------------------------------------------------------------------
// Webhook config bridging
// ---------------------------------------------------------------------------

#[test]
fn webhook_url_only_override_preserves_file_secret_and_timeout() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        webhook_url: Some("https://new-hook.example".to_string()),
        ..Default::default()
    }));

    assert_eq!(rt.webhook.url, "https://new-hook.example");
    assert_eq!(rt.webhook.secret.as_deref(), Some("file-secret"));
    assert_eq!(rt.webhook.timeout_secs, 45);
}

#[test]
fn webhook_secret_only_override_preserves_file_url() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        webhook_secret: Some("cli-secret".to_string()),
        ..Default::default()
    }));

    assert_eq!(rt.webhook.url, "https://hooks.custom.local");
    assert_eq!(rt.webhook.secret.as_deref(), Some("cli-secret"));
}

#[test]
fn webhook_no_overrides_uses_file_config() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides::default()));

    assert_eq!(rt.webhook.url, "https://hooks.custom.local");
    assert_eq!(rt.webhook.secret.as_deref(), Some("file-secret"));
    assert_eq!(rt.webhook.timeout_secs, 45);
}

#[test]
fn webhook_default_config_no_overrides_yields_empty() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));

    assert!(rt.webhook.url.is_empty());
    assert!(rt.webhook.secret.is_none());
    assert_eq!(rt.webhook.timeout_secs, 30); // default timeout
}

// ---------------------------------------------------------------------------
// Encryption config bridging
// ---------------------------------------------------------------------------

#[test]
fn encryption_cli_encrypt_flag_enables() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        encrypt: true,
        ..Default::default()
    }));

    assert!(rt.encryption.enabled);
    // No passphrase from either source → env_var fallback
    assert_eq!(
        rt.encryption.env_var.as_deref(),
        Some("SHIPPER_ENCRYPT_KEY")
    );
}

#[test]
fn encryption_cli_passphrase_overrides_config_passphrase() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        encrypt_passphrase: Some("cli-pass".to_string()),
        ..Default::default()
    }));

    assert!(rt.encryption.enabled);
    assert_eq!(rt.encryption.passphrase.as_deref(), Some("cli-pass"));
}

#[test]
fn encryption_config_passphrase_used_when_no_cli_passphrase() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides::default()));

    assert!(rt.encryption.enabled);
    assert_eq!(rt.encryption.passphrase.as_deref(), Some("file-passphrase"));
}

#[test]
fn encryption_env_key_from_config_used() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides::default()));

    assert_eq!(rt.encryption.env_var.as_deref(), Some("CUSTOM_KEY"));
}

#[test]
fn encryption_disabled_when_neither_cli_nor_config_enables() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));
    assert!(!rt.encryption.enabled);
}

// ---------------------------------------------------------------------------
// Registry selection
// ---------------------------------------------------------------------------

#[test]
fn no_registry_flags_yields_empty_registries() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));
    assert!(rt.registries.is_empty());
}

#[test]
fn all_registries_flag_returns_configured_registries() {
    let mut cfg = default_config();
    cfg.registries = MultiRegistryConfig {
        registries: vec![
            RegistryConfig {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: Some("https://index.crates.io".to_string()),
                token: None,
                default: true,
            },
            RegistryConfig {
                name: "private".to_string(),
                api_base: "https://private.example".to_string(),
                index_base: None,
                token: None,
                default: false,
            },
        ],
        default_registries: vec![],
    };

    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
        all_registries: true,
        ..Default::default()
    }));

    assert_eq!(rt.registries.len(), 2);
    assert_eq!(rt.registries[0].name, "crates-io");
    assert_eq!(rt.registries[1].name, "private");
}

#[test]
fn all_registries_with_empty_config_returns_default_crates_io() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        all_registries: true,
        ..Default::default()
    }));

    assert_eq!(rt.registries.len(), 1);
    assert_eq!(rt.registries[0].name, "crates-io");
    assert_eq!(rt.registries[0].api_base, "https://crates.io");
}

#[test]
fn specific_registry_names_selected_from_config() {
    let mut cfg = default_config();
    cfg.registries = MultiRegistryConfig {
        registries: vec![
            RegistryConfig {
                name: "alpha".to_string(),
                api_base: "https://alpha.example".to_string(),
                index_base: None,
                token: None,
                default: false,
            },
            RegistryConfig {
                name: "beta".to_string(),
                api_base: "https://beta.example".to_string(),
                index_base: None,
                token: None,
                default: false,
            },
        ],
        default_registries: vec![],
    };

    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides {
        registries: Some(vec!["beta".to_string()]),
        ..Default::default()
    }));

    assert_eq!(rt.registries.len(), 1);
    assert_eq!(rt.registries[0].name, "beta");
    assert_eq!(rt.registries[0].api_base, "https://beta.example");
}

#[test]
fn unknown_registry_name_falls_back_to_synthetic() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        registries: Some(vec!["unknown-reg".to_string()]),
        ..Default::default()
    }));

    assert_eq!(rt.registries.len(), 1);
    assert_eq!(rt.registries[0].name, "unknown-reg");
}

#[test]
fn crates_io_name_falls_back_to_well_known_registry() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        registries: Some(vec!["crates-io".to_string()]),
        ..Default::default()
    }));

    assert_eq!(rt.registries.len(), 1);
    assert_eq!(rt.registries[0].name, "crates-io");
    assert_eq!(rt.registries[0].api_base, "https://crates.io");
    assert_eq!(
        rt.registries[0].index_base.as_deref(),
        Some("https://index.crates.io")
    );
}

// ---------------------------------------------------------------------------
// Retry config overrides and policy precedence
// ---------------------------------------------------------------------------

#[test]
fn retry_cli_max_attempts_overrides_policy_default() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        max_attempts: Some(99),
        ..Default::default()
    }));
    assert_eq!(rt.max_attempts, 99);
}

#[test]
fn retry_cli_base_delay_and_max_delay_override() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        base_delay: Some(Duration::from_millis(100)),
        max_delay: Some(Duration::from_secs(10)),
        ..Default::default()
    }));
    assert_eq!(rt.base_delay, Duration::from_millis(100));
    assert_eq!(rt.max_delay, Duration::from_secs(10));
}

#[test]
fn retry_cli_strategy_and_jitter_override() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        retry_strategy: Some(RetryStrategyType::Linear),
        retry_jitter: Some(0.1),
        ..Default::default()
    }));
    assert_eq!(rt.retry_strategy, RetryStrategyType::Linear);
    assert!((rt.retry_jitter - 0.1).abs() < f64::EPSILON);
}

#[test]
fn aggressive_retry_policy_defaults_without_cli_overrides() {
    let mut cfg = default_config();
    cfg.retry.policy = shipper_retry::RetryPolicy::Aggressive;

    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides::default()));

    // Aggressive: max_attempts=10, base_delay=500ms, max_delay=30s, jitter=0.3
    assert_eq!(rt.max_attempts, 10);
    assert_eq!(rt.base_delay, Duration::from_millis(500));
    assert_eq!(rt.max_delay, Duration::from_secs(30));
    assert!((rt.retry_jitter - 0.3).abs() < f64::EPSILON);
}

#[test]
fn conservative_retry_policy_defaults_without_cli_overrides() {
    let mut cfg = default_config();
    cfg.retry.policy = shipper_retry::RetryPolicy::Conservative;

    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides::default()));

    assert_eq!(rt.max_attempts, 3);
    assert_eq!(rt.base_delay, Duration::from_secs(5));
    assert_eq!(rt.max_delay, Duration::from_secs(60));
    assert_eq!(rt.retry_strategy, RetryStrategyType::Linear);
}

#[test]
fn custom_retry_policy_uses_explicit_config_fields() {
    let mut cfg = default_config();
    cfg.retry = shipper_config::RetryConfig {
        policy: shipper_retry::RetryPolicy::Custom,
        max_attempts: 2,
        base_delay: Duration::from_millis(50),
        max_delay: Duration::from_secs(5),
        strategy: RetryStrategyType::Linear,
        jitter: 0.05,
        per_error: shipper_retry::PerErrorConfig::default(),
    };

    let rt = into_runtime_options(cfg.build_runtime_options(CliOverrides::default()));

    assert_eq!(rt.max_attempts, 2);
    assert_eq!(rt.base_delay, Duration::from_millis(50));
    assert_eq!(rt.max_delay, Duration::from_secs(5));
    assert_eq!(rt.retry_strategy, RetryStrategyType::Linear);
    assert!((rt.retry_jitter - 0.05).abs() < f64::EPSILON);
}

// ---------------------------------------------------------------------------
// Verify timeout / poll interval defaults
// ---------------------------------------------------------------------------

#[test]
fn verify_timeout_defaults_when_no_cli_override() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));
    assert_eq!(rt.verify_timeout, Duration::from_secs(120));
    assert_eq!(rt.verify_poll_interval, Duration::from_secs(5));
}

#[test]
fn verify_timeout_cli_override() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        verify_timeout: Some(Duration::from_secs(60)),
        verify_poll_interval: Some(Duration::from_secs(1)),
        ..Default::default()
    }));
    assert_eq!(rt.verify_timeout, Duration::from_secs(60));
    assert_eq!(rt.verify_poll_interval, Duration::from_secs(1));
}

// ---------------------------------------------------------------------------
// resume_from
// ---------------------------------------------------------------------------

#[test]
fn resume_from_passed_through() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides {
        resume_from: Some("my-crate".to_string()),
        ..Default::default()
    }));
    assert_eq!(rt.resume_from.as_deref(), Some("my-crate"));
}

#[test]
fn resume_from_none_by_default() {
    let rt = into_runtime_options(default_config().build_runtime_options(CliOverrides::default()));
    assert!(rt.resume_from.is_none());
}

// ---------------------------------------------------------------------------
// Combined: multiple partial CLI overrides at once
// ---------------------------------------------------------------------------

#[test]
fn multiple_partial_overrides_merge_correctly() {
    let rt = into_runtime_options(custom_config().build_runtime_options(CliOverrides {
        // Override only a few fields; the rest should come from custom_config.
        policy: Some(PublishPolicy::Safe),
        max_concurrent: Some(2),
        webhook_secret: Some("new-secret".to_string()),
        lock_timeout: Some(Duration::from_secs(100)),
        no_readiness: true,
        ..Default::default()
    }));

    // Overridden
    assert_eq!(rt.policy, PublishPolicy::Safe);
    assert_eq!(rt.parallel.max_concurrent, 2);
    assert_eq!(rt.webhook.secret.as_deref(), Some("new-secret"));
    assert_eq!(rt.lock_timeout, Duration::from_secs(100));
    assert!(!rt.readiness.enabled);

    // From config file (not overridden)
    assert_eq!(rt.verify_mode, VerifyMode::None);
    assert_eq!(rt.output_lines, 300);
    assert_eq!(rt.webhook.url, "https://hooks.custom.local");
    assert_eq!(rt.webhook.timeout_secs, 45);
    assert!(rt.parallel.enabled);
    assert_eq!(rt.state_dir, PathBuf::from("custom-state"));
    // Flags are OR'd: config has all true, CLI has all false → true
    assert!(rt.allow_dirty);
    assert!(rt.skip_ownership_check);
    assert!(rt.strict_ownership);
}
