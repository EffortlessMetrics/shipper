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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DEFAULT_MAX_ATTEMPTS: u32 = 6;

    fn make_config(policy: RetryPolicy) -> RetryConfig {
        let mut config = RetryConfig {
            policy,
            max_attempts: 999,
            base_delay: Duration::from_secs(999),
            max_delay: Duration::from_secs(999),
            strategy: RetryStrategyType::Constant,
            jitter: 0.9,
            per_error: PerErrorConfig::default(),
        };
        if policy == RetryPolicy::Custom {
            config.max_attempts = TEST_DEFAULT_MAX_ATTEMPTS;
        }
        config
    }

    #[test]
    fn resolve_uses_policy_defaults_when_policy_is_preset() {
        let config = make_config(RetryPolicy::Default);
        let cli = CliOverrides::default();
        let policy_defaults = config.policy.to_config();

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, policy_defaults.max_attempts);
        assert_eq!(resolved.base_delay, policy_defaults.base_delay);
        assert_eq!(resolved.max_delay, policy_defaults.max_delay);
        assert_eq!(resolved.strategy, policy_defaults.strategy);
        assert!((resolved.jitter - policy_defaults.jitter).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_aggressive_policy_overrides_field_values() {
        let config = make_config(RetryPolicy::Aggressive);
        let cli = CliOverrides::default();
        let policy_defaults = config.policy.to_config();

        let resolved = resolve(&config, &cli);

        assert_eq!(
            resolved.max_attempts, policy_defaults.max_attempts,
            "preset policy should ignore the explicit max_attempts field"
        );
        assert_eq!(resolved.base_delay, policy_defaults.base_delay);
        assert_eq!(resolved.strategy, policy_defaults.strategy);
    }

    #[test]
    fn resolve_custom_policy_uses_explicit_field_values() {
        let config = RetryConfig {
            policy: RetryPolicy::Custom,
            max_attempts: 11,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(7),
            strategy: RetryStrategyType::Linear,
            jitter: 0.25,
            per_error: PerErrorConfig::default(),
        };
        let cli = CliOverrides::default();

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, 11);
        assert_eq!(resolved.base_delay, Duration::from_millis(250));
        assert_eq!(resolved.max_delay, Duration::from_secs(7));
        assert_eq!(resolved.strategy, RetryStrategyType::Linear);
        assert!((resolved.jitter - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_cli_overrides_take_priority_over_preset_policy() {
        let config = make_config(RetryPolicy::Default);
        let cli = CliOverrides {
            max_attempts: Some(42),
            base_delay: Some(Duration::from_secs(3)),
            max_delay: Some(Duration::from_secs(33)),
            retry_strategy: Some(RetryStrategyType::Immediate),
            retry_jitter: Some(0.1),
            ..Default::default()
        };

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, 42);
        assert_eq!(resolved.base_delay, Duration::from_secs(3));
        assert_eq!(resolved.max_delay, Duration::from_secs(33));
        assert_eq!(resolved.strategy, RetryStrategyType::Immediate);
        assert!((resolved.jitter - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_partial_cli_overrides_keep_preset_policy_defaults_for_unset_fields() {
        let config = make_config(RetryPolicy::Conservative);
        let cli = CliOverrides {
            max_attempts: Some(7),
            ..Default::default()
        };
        let policy_defaults = config.policy.to_config();

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, 7);
        assert_eq!(resolved.base_delay, policy_defaults.base_delay);
        assert_eq!(resolved.max_delay, policy_defaults.max_delay);
        assert_eq!(resolved.strategy, policy_defaults.strategy);
        assert!((resolved.jitter - policy_defaults.jitter).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_cli_overrides_take_priority_over_custom_policy() {
        let config = RetryConfig {
            policy: RetryPolicy::Custom,
            max_attempts: 11,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(7),
            strategy: RetryStrategyType::Linear,
            jitter: 0.25,
            per_error: PerErrorConfig::default(),
        };
        let cli = CliOverrides {
            max_attempts: Some(5),
            retry_strategy: Some(RetryStrategyType::Exponential),
            ..Default::default()
        };

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, 5);
        assert_eq!(resolved.strategy, RetryStrategyType::Exponential);
        assert_eq!(
            resolved.base_delay,
            Duration::from_millis(250),
            "non-overridden fields keep the custom-policy value"
        );
    }

    #[test]
    fn resolve_preserves_per_error_config_verbatim() {
        let retryable = shipper_retry::RetryStrategyConfig {
            strategy: RetryStrategyType::Immediate,
            max_attempts: 7,
            base_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(1),
            jitter: 0.0,
        };
        let per_error = PerErrorConfig {
            retryable: Some(retryable),
            ambiguous: None,
            permanent: None,
        };

        let config = RetryConfig {
            policy: RetryPolicy::Default,
            max_attempts: TEST_DEFAULT_MAX_ATTEMPTS,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
            strategy: RetryStrategyType::Exponential,
            jitter: 0.5,
            per_error: per_error.clone(),
        };
        let cli = CliOverrides::default();

        let resolved = resolve(&config, &cli);

        let resolved_retryable = resolved
            .per_error
            .retryable
            .as_ref()
            .expect("retryable settings should round-trip");
        assert_eq!(resolved_retryable.strategy, RetryStrategyType::Immediate);
        assert_eq!(resolved_retryable.max_attempts, 7);
        assert_eq!(resolved_retryable.base_delay, Duration::from_millis(50));
        assert!(resolved.per_error.ambiguous.is_none());
        assert!(resolved.per_error.permanent.is_none());
    }
}
