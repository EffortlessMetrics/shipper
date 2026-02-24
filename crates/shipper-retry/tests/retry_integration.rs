use std::time::Duration;

use shipper_retry::{
    config_for_error,
    ErrorClass,
    PerErrorConfig,
    RetryExecutor,
    RetryPolicy,
    RetryStrategyConfig,
    RetryStrategyType,
};

#[test]
fn integration_config_for_error_uses_per_error_override() {
    let default = RetryStrategyConfig::default();
    let override_config = RetryStrategyConfig {
        strategy: RetryStrategyType::Immediate,
        max_attempts: 12,
        base_delay: Duration::from_millis(12),
        max_delay: Duration::from_millis(120),
        jitter: 0.0,
    };

    let per_error = PerErrorConfig {
        retryable: Some(override_config.clone()),
        ambiguous: None,
        permanent: None,
    };

    let result = config_for_error(&default, Some(&per_error), ErrorClass::Retryable);
    assert_eq!(result.max_attempts, 12);
    assert_eq!(result.strategy, RetryStrategyType::Immediate);
}

#[test]
fn integration_executor_executes_with_backoff_config() {
    let executor = RetryExecutor::new(RetryStrategyConfig {
        strategy: RetryStrategyType::Immediate,
        max_attempts: 2,
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        jitter: 0.0,
    });

    let mut attempts = 0u32;
    let result = executor.run(|attempt| {
        attempts = attempt;
        if attempt < 2 {
            Err("retry")
        } else {
            Ok(())
        }
    });

    assert!(result.is_ok());
    assert_eq!(attempts, 2);
}

#[test]
fn integration_executor_respects_classic_policy_defaults() {
    let cfg = RetryPolicy::Default.to_config();
    assert_eq!(cfg.strategy, RetryStrategyType::Exponential);
    assert_eq!(cfg.max_attempts, 6);
    assert_eq!(cfg.base_delay, Duration::from_secs(2));
    assert_eq!(cfg.max_delay, Duration::from_secs(120));
}
