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

    fn empty_cli() -> CliOverrides {
        CliOverrides::default()
    }

    fn config_with_policy(policy: RetryPolicy) -> RetryConfig {
        RetryConfig {
            policy,
            ..RetryConfig::default()
        }
    }

    #[test]
    fn select_policy_value_returns_policy_value_when_not_custom() {
        let selected = select_policy_value(false, 10u32, 5u32);
        assert_eq!(selected, 5);
    }

    #[test]
    fn select_policy_value_returns_custom_value_when_custom_policy() {
        let selected = select_policy_value(true, 10u32, 5u32);
        assert_eq!(selected, 10);
    }

    #[test]
    fn select_policy_value_works_with_duration() {
        let custom = Duration::from_secs(7);
        let policy = Duration::from_secs(3);

        assert_eq!(select_policy_value(true, custom, policy), custom);
        assert_eq!(select_policy_value(false, custom, policy), policy);
    }

    #[test]
    fn resolve_default_policy_uses_policy_defaults_not_custom_fields() {
        // Set custom fields that should be IGNORED because policy != Custom.
        let config = RetryConfig {
            policy: RetryPolicy::Default,
            max_attempts: 999,
            base_delay: Duration::from_secs(99),
            ..RetryConfig::default()
        };

        let policy_defaults = RetryPolicy::Default.to_config();

        let resolved = resolve(&config, &empty_cli());

        assert_eq!(resolved.max_attempts, policy_defaults.max_attempts);
        assert_eq!(resolved.base_delay, policy_defaults.base_delay);
        assert_ne!(resolved.max_attempts, 999);
    }

    #[test]
    fn resolve_aggressive_policy_uses_aggressive_defaults() {
        let config = config_with_policy(RetryPolicy::Aggressive);
        let policy_defaults = RetryPolicy::Aggressive.to_config();

        let resolved = resolve(&config, &empty_cli());

        assert_eq!(resolved.max_attempts, policy_defaults.max_attempts);
        assert_eq!(resolved.base_delay, policy_defaults.base_delay);
        assert_eq!(resolved.max_delay, policy_defaults.max_delay);
        assert_eq!(resolved.strategy, policy_defaults.strategy);
        assert_eq!(resolved.jitter, policy_defaults.jitter);
    }

    #[test]
    fn resolve_conservative_policy_uses_conservative_defaults() {
        let config = config_with_policy(RetryPolicy::Conservative);
        let policy_defaults = RetryPolicy::Conservative.to_config();

        let resolved = resolve(&config, &empty_cli());

        assert_eq!(resolved.max_attempts, policy_defaults.max_attempts);
        assert_eq!(resolved.base_delay, policy_defaults.base_delay);
    }

    #[test]
    fn resolve_custom_policy_uses_config_fields_directly() {
        let config = RetryConfig {
            policy: RetryPolicy::Custom,
            max_attempts: 17,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(45),
            jitter: 0.25,
            ..RetryConfig::default()
        };

        let resolved = resolve(&config, &empty_cli());

        assert_eq!(resolved.max_attempts, 17);
        assert_eq!(resolved.base_delay, Duration::from_millis(250));
        assert_eq!(resolved.max_delay, Duration::from_secs(45));
        assert_eq!(resolved.jitter, 0.25);
    }

    #[test]
    fn resolve_cli_max_attempts_overrides_policy_default() {
        let config = config_with_policy(RetryPolicy::Default);
        let mut cli = empty_cli();
        cli.max_attempts = Some(42);

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, 42);
    }

    #[test]
    fn resolve_cli_max_attempts_overrides_custom_policy() {
        let config = RetryConfig {
            policy: RetryPolicy::Custom,
            max_attempts: 17,
            ..RetryConfig::default()
        };

        let mut cli = empty_cli();
        cli.max_attempts = Some(99);

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, 99);
    }

    #[test]
    fn resolve_cli_base_delay_overrides_policy() {
        let config = config_with_policy(RetryPolicy::Default);
        let mut cli = empty_cli();
        cli.base_delay = Some(Duration::from_millis(123));

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.base_delay, Duration::from_millis(123));
    }

    #[test]
    fn resolve_cli_max_delay_overrides_policy() {
        let config = config_with_policy(RetryPolicy::Default);
        let mut cli = empty_cli();
        cli.max_delay = Some(Duration::from_secs(300));

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_delay, Duration::from_secs(300));
    }

    #[test]
    fn resolve_cli_strategy_overrides_policy() {
        let config = config_with_policy(RetryPolicy::Default);
        let mut cli = empty_cli();
        cli.retry_strategy = Some(RetryStrategyType::Linear);

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.strategy, RetryStrategyType::Linear);
    }

    #[test]
    fn resolve_cli_jitter_overrides_policy() {
        let config = config_with_policy(RetryPolicy::Default);
        let mut cli = empty_cli();
        cli.retry_jitter = Some(0.75);

        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.jitter, 0.75);
    }

    #[test]
    fn resolve_per_error_is_cloned_through_unchanged() {
        let config = RetryConfig {
            per_error: PerErrorConfig::default(),
            ..RetryConfig::default()
        };

        let resolved = resolve(&config, &empty_cli());

        assert_eq!(
            format!("{:?}", resolved.per_error),
            format!("{:?}", config.per_error)
        );
    }

    #[test]
    fn resolve_cli_overrides_take_precedence_independently() {
        // Mixed: some CLI overrides set, others unset; non-set should follow policy.
        let config = config_with_policy(RetryPolicy::Conservative);
        let mut cli = empty_cli();
        cli.max_attempts = Some(7);
        // base_delay, max_delay, strategy, jitter all None — should follow Conservative.

        let policy_defaults = RetryPolicy::Conservative.to_config();
        let resolved = resolve(&config, &cli);

        assert_eq!(resolved.max_attempts, 7);
        assert_eq!(resolved.base_delay, policy_defaults.base_delay);
        assert_eq!(resolved.max_delay, policy_defaults.max_delay);
        assert_eq!(resolved.strategy, policy_defaults.strategy);
        assert_eq!(resolved.jitter, policy_defaults.jitter);
    }
}
