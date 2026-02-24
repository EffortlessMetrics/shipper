pub use shipper_config::*;
pub use shipper_config_runtime::*;

#[cfg(not(feature = "micro-types"))]
impl From<shipper_config::PublishPolicy> for crate::types::PublishPolicy {
    fn from(value: shipper_config::PublishPolicy) -> Self {
        match value {
            shipper_config::PublishPolicy::Safe => crate::types::PublishPolicy::Safe,
            shipper_config::PublishPolicy::Balanced => crate::types::PublishPolicy::Balanced,
            shipper_config::PublishPolicy::Fast => crate::types::PublishPolicy::Fast,
        }
    }
}

#[cfg(not(feature = "micro-types"))]
impl From<shipper_config::VerifyMode> for crate::types::VerifyMode {
    fn from(value: shipper_config::VerifyMode) -> Self {
        match value {
            shipper_config::VerifyMode::Workspace => crate::types::VerifyMode::Workspace,
            shipper_config::VerifyMode::Package => crate::types::VerifyMode::Package,
            shipper_config::VerifyMode::None => crate::types::VerifyMode::None,
        }
    }
}

#[cfg(not(feature = "micro-types"))]
impl From<shipper_config::ReadinessMethod> for crate::types::ReadinessMethod {
    fn from(value: shipper_config::ReadinessMethod) -> Self {
        match value {
            shipper_config::ReadinessMethod::Api => crate::types::ReadinessMethod::Api,
            shipper_config::ReadinessMethod::Index => crate::types::ReadinessMethod::Index,
            shipper_config::ReadinessMethod::Both => crate::types::ReadinessMethod::Both,
        }
    }
}

#[cfg(not(feature = "micro-types"))]
impl From<shipper_config::ReadinessConfig> for crate::types::ReadinessConfig {
    fn from(value: shipper_config::ReadinessConfig) -> Self {
        Self {
            enabled: value.enabled,
            method: value.method.into(),
            initial_delay: value.initial_delay,
            max_delay: value.max_delay,
            max_total_wait: value.max_total_wait,
            poll_interval: value.poll_interval,
            jitter_factor: value.jitter_factor,
            index_path: value.index_path,
            prefer_index: value.prefer_index,
        }
    }
}

#[cfg(not(feature = "micro-types"))]
impl From<shipper_config::ParallelConfig> for crate::types::ParallelConfig {
    fn from(value: shipper_config::ParallelConfig) -> Self {
        Self {
            enabled: value.enabled,
            max_concurrent: value.max_concurrent,
            per_package_timeout: value.per_package_timeout,
        }
    }
}

#[cfg(not(feature = "micro-types"))]
impl From<shipper_config::Registry> for crate::types::Registry {
    fn from(value: shipper_config::Registry) -> Self {
        Self {
            name: value.name,
            api_base: value.api_base,
            index_base: value.index_base,
        }
    }
}

#[cfg(not(feature = "micro-types"))]
impl From<shipper_config::RuntimeOptions> for crate::types::RuntimeOptions {
    fn from(value: shipper_config::RuntimeOptions) -> Self {
        let converted = shipper_config_runtime::into_runtime_options(value);

        Self {
            allow_dirty: converted.allow_dirty,
            skip_ownership_check: converted.skip_ownership_check,
            strict_ownership: converted.strict_ownership,
            no_verify: converted.no_verify,
            max_attempts: converted.max_attempts,
            base_delay: converted.base_delay,
            max_delay: converted.max_delay,
            retry_strategy: converted.retry_strategy,
            retry_jitter: converted.retry_jitter,
            retry_per_error: converted.retry_per_error,
            verify_timeout: converted.verify_timeout,
            verify_poll_interval: converted.verify_poll_interval,
            state_dir: converted.state_dir,
            force_resume: converted.force_resume,
            policy: converted.policy.into(),
            verify_mode: converted.verify_mode.into(),
            readiness: converted.readiness.into(),
            output_lines: converted.output_lines,
            force: converted.force,
            lock_timeout: converted.lock_timeout,
            parallel: converted.parallel.into(),
            webhook: crate::webhook::WebhookConfig {
                enabled: !converted.webhook.url.trim().is_empty(),
                url: Some(converted.webhook.url),
                secret: converted.webhook.secret,
                timeout: std::time::Duration::from_secs(converted.webhook.timeout_secs),
            },
            encryption: crate::encryption::EncryptionConfig {
                enabled: converted.encryption.enabled,
                passphrase: converted.encryption.passphrase,
                env_var: converted.encryption.env_var,
            },
            registries: converted.registries.into_iter().map(Into::into).collect(),
        }
    }
}
