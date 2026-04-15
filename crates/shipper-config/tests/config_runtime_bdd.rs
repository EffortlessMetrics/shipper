use std::path::PathBuf;
use std::time::Duration;

use shipper_config::runtime::into_runtime_options;
use shipper_config::{
    EncryptionConfig, ParallelConfig, PublishPolicy, ReadinessConfig, ReadinessMethod, Registry,
    RuntimeOptions, VerifyMode, WebhookConfig,
};

fn sample_runtime_options(base_url: &str, registry_count: usize) -> RuntimeOptions {
    RuntimeOptions {
        allow_dirty: true,
        skip_ownership_check: true,
        strict_ownership: false,
        no_verify: false,
        max_attempts: 9,
        base_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(30),
        retry_strategy: shipper_retry::RetryStrategyType::Linear,
        retry_jitter: 0.35,
        retry_per_error: shipper_retry::PerErrorConfig::default(),
        verify_timeout: Duration::from_secs(120),
        verify_poll_interval: Duration::from_secs(5),
        state_dir: PathBuf::from(".shipper"),
        force_resume: false,
        policy: PublishPolicy::Balanced,
        verify_mode: VerifyMode::Package,
        readiness: ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::from_millis(150),
            max_delay: Duration::from_secs(20),
            max_total_wait: Duration::from_secs(150),
            poll_interval: Duration::from_secs(4),
            jitter_factor: 0.25,
            index_path: None,
            prefer_index: true,
        },
        output_lines: 160,
        force: false,
        lock_timeout: Duration::from_secs(600),
        parallel: ParallelConfig {
            enabled: true,
            max_concurrent: 4,
            per_package_timeout: Duration::from_secs(20),
        },
        webhook: WebhookConfig {
            url: base_url.to_string(),
            webhook_type: Default::default(),
            secret: Some("top-secret".to_string()),
            timeout_secs: 90,
        },
        encryption: EncryptionConfig {
            enabled: true,
            passphrase: Some("s3cr3t".to_string()),
            env_var: None,
        },
        registries: (0..registry_count)
            .map(|idx| Registry {
                name: format!("r{idx}"),
                api_base: format!("https://r{idx}.example"),
                index_base: None,
            })
            .collect(),
        resume_from: None,
    }
}
#[test]
fn bdd_given_a_webhook_url_when_converted_then_webhook_payload_preserves_timeout_and_secret() {
    let input = sample_runtime_options("https://hooks.example.local/hook", 1);
    let converted = into_runtime_options(input);

    assert_eq!(converted.webhook.url, "https://hooks.example.local/hook");
    assert_eq!(converted.webhook.timeout_secs, 90);
    assert_eq!(converted.webhook.secret.as_deref(), Some("top-secret"));
}

#[test]
fn bdd_given_whitespace_webhook_url_when_converted_then_is_preserved_for_forwarded_send() {
    let input = sample_runtime_options("   ", 2);
    let converted = into_runtime_options(input);

    assert_eq!(converted.webhook.url, "   ");
    assert_eq!(converted.webhook.secret.as_deref(), Some("top-secret"));
}

#[test]
fn bdd_given_empty_webhook_url_when_converted_then_registry_and_parallel_shape_is_kept() {
    let input = sample_runtime_options("", 3);
    let converted = into_runtime_options(input);

    assert_eq!(converted.webhook.url, "");
    assert_eq!(converted.parallel.max_concurrent, 4);
    assert_eq!(converted.registries.len(), 3);
}
