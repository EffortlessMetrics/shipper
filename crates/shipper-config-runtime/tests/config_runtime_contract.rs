use std::path::PathBuf;
use std::time::Duration;

use shipper_config::{CliOverrides, PolicyConfig, ReadinessConfig, ReadinessMethod, ShipperConfig};
use shipper_config_runtime::into_runtime_options;
use shipper_types::{ParallelConfig, PublishPolicy, VerifyMode};

#[test]
fn converts_config_to_runtime_contract_with_registry_overrides() {
    let source = ShipperConfig {
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
