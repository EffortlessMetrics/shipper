use std::time::Duration;

use shipper_retry::{PerErrorConfig, RetryPolicy, RetryStrategyType};

use crate::{CliOverrides, RetryConfig};

pub(super) struct ResolvedRetry {
    pub(super) max_attempts: u32,
    pub(super) base_delay: Duration,
    pub(super) max_delay: Duration,
    pub(super) strategy: RetryStrategyType,
    pub(super) jitter: f64,
    pub(super) per_error: PerErrorConfig,
}

pub(super) fn resolve(config: &RetryConfig, cli: &CliOverrides) -> ResolvedRetry {
    let policy_defaults = config.policy.to_config();
    let custom_policy = config.policy == RetryPolicy::Custom;

    ResolvedRetry {
        max_attempts: cli.max_attempts.unwrap_or(select_policy_value(
            custom_policy,
            config.max_attempts,
            policy_defaults.max_attempts,
        )),
        base_delay: cli.base_delay.unwrap_or(select_policy_value(
            custom_policy,
            config.base_delay,
            policy_defaults.base_delay,
        )),
        max_delay: cli.max_delay.unwrap_or(select_policy_value(
            custom_policy,
            config.max_delay,
            policy_defaults.max_delay,
        )),
        strategy: cli.retry_strategy.unwrap_or(select_policy_value(
            custom_policy,
            config.strategy,
            policy_defaults.strategy,
        )),
        jitter: cli.retry_jitter.unwrap_or(select_policy_value(
            custom_policy,
            config.jitter,
            policy_defaults.jitter,
        )),
        per_error: config.per_error.clone(),
    }
}

fn select_policy_value<T>(custom_policy: bool, custom_value: T, policy_value: T) -> T {
    if custom_policy {
        custom_value
    } else {
        policy_value
    }
}
