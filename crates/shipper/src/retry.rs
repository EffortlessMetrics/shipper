//! Retry strategy module for configurable retry behaviors.
//!
//! This module provides retry strategies that can be configured based on error types,
//! allowing users to customize retry behavior for different failure scenarios.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::ErrorClass;

/// Strategy type for retry behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryStrategyType {
    /// No delay between retries - retry immediately
    Immediate,
    /// Exponential backoff: delay doubles each attempt (default)
    #[default]
    Exponential,
    /// Linear backoff: delay increases linearly each attempt
    Linear,
    /// Constant delay: same delay every attempt
    Constant,
}

/// Predefined retry policies with sensible defaults for different use cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryPolicy {
    /// Default balanced retry behavior - good for most scenarios
    #[default]
    Default,
    /// Aggressive retries - more attempts, faster recovery
    Aggressive,
    /// Conservative retries - fewer attempts, longer delays
    Conservative,
    /// Fully custom configuration via retry.strategy settings
    Custom,
}

impl RetryPolicy {
    /// Get the default retry configuration for this policy.
    pub fn to_config(&self) -> RetryStrategyConfig {
        match self {
            RetryPolicy::Default => RetryStrategyConfig {
                strategy: RetryStrategyType::Exponential,
                max_attempts: 6,
                base_delay: Duration::from_secs(2),
                max_delay: Duration::from_secs(120),
                jitter: 0.5,
            },
            RetryPolicy::Aggressive => RetryStrategyConfig {
                strategy: RetryStrategyType::Exponential,
                max_attempts: 10,
                base_delay: Duration::from_millis(500),
                max_delay: Duration::from_secs(30),
                jitter: 0.3,
            },
            RetryPolicy::Conservative => RetryStrategyConfig {
                strategy: RetryStrategyType::Linear,
                max_attempts: 3,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(60),
                jitter: 0.1,
            },
            RetryPolicy::Custom => {
                // Custom uses the explicitly configured values
                RetryStrategyConfig::default()
            }
        }
    }
}

/// Configuration for a retry strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryStrategyConfig {
    /// Strategy type for calculating delay between retries.
    #[serde(default)]
    pub strategy: RetryStrategyType,
    /// Maximum number of retry attempts.
    #[serde(default)]
    pub max_attempts: u32,
    /// Base delay for backoff calculations.
    #[serde(
        deserialize_with = "crate::types::deserialize_duration",
        serialize_with = "crate::types::serialize_duration"
    )]
    #[serde(default)]
    pub base_delay: Duration,
    /// Maximum delay cap for backoff.
    #[serde(
        deserialize_with = "crate::types::deserialize_duration",
        serialize_with = "crate::types::serialize_duration"
    )]
    #[serde(default)]
    pub max_delay: Duration,
    /// Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter).
    #[serde(default = "default_jitter")]
    pub jitter: f64,
}

impl Default for RetryStrategyConfig {
    fn default() -> Self {
        Self {
            strategy: RetryStrategyType::Exponential,
            max_attempts: 6,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
            jitter: 0.5,
        }
    }
}

fn default_jitter() -> f64 {
    0.5
}

/// Per-error-type retry configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerErrorConfig {
    /// Retry configuration for retryable errors (e.g., network issues, rate limiting).
    #[serde(default, rename = "retryable")]
    pub retryable: Option<RetryStrategyConfig>,
    /// Retry configuration for ambiguous errors (e.g., unknown if publish succeeded).
    #[serde(default, rename = "ambiguous")]
    pub ambiguous: Option<RetryStrategyConfig>,
    /// Retry configuration for permanent errors (e.g., authentication failure).
    /// Permanent errors are typically not retried, but this can be customized.
    #[serde(default, rename = "permanent")]
    pub permanent: Option<RetryStrategyConfig>,
}

/// Calculate the delay for the next retry attempt based on the strategy configuration.
pub fn calculate_delay(config: &RetryStrategyConfig, attempt: u32) -> Duration {
    let delay = match config.strategy {
        RetryStrategyType::Immediate => Duration::ZERO,
        RetryStrategyType::Exponential => {
            let pow = attempt.saturating_sub(1).min(16);
            config.base_delay.saturating_mul(2_u32.saturating_pow(pow))
        }
        RetryStrategyType::Linear => config.base_delay.saturating_mul(attempt),
        RetryStrategyType::Constant => config.base_delay,
    };

    // Cap at max_delay
    let capped = delay.min(config.max_delay);

    // Apply jitter if enabled
    if config.jitter > 0.0 {
        apply_jitter(capped, config.jitter)
    } else {
        capped
    }
}

/// Apply jitter to a delay value.
/// Jitter factor of 0.5 means delay * (0.5 to 1.5).
fn apply_jitter(delay: Duration, jitter: f64) -> Duration {
    // Generate a random factor between (1 - jitter) and (1 + jitter)
    let jitter_range = 2.0 * jitter;
    let random_factor = 1.0 - jitter + (rand::random::<f64>() * jitter_range);
    let millis = (delay.as_millis() as f64 * random_factor).round() as u64;
    Duration::from_millis(millis)
}

/// Get the retry configuration for a specific error class.
/// Falls back to the default config if no per-error config is specified.
pub fn config_for_error(
    default_config: &RetryStrategyConfig,
    per_error_config: &Option<PerErrorConfig>,
    error_class: ErrorClass,
) -> RetryStrategyConfig {
    if let Some(per_error) = per_error_config {
        match error_class {
            ErrorClass::Retryable => {
                if let Some(config) = &per_error.retryable {
                    return config.clone();
                }
            }
            ErrorClass::Ambiguous => {
                if let Some(config) = &per_error.ambiguous {
                    return config.clone();
                }
            }
            ErrorClass::Permanent => {
                if let Some(config) = &per_error.permanent {
                    return config.clone();
                }
            }
        }
    }
    default_config.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_to_config_default() {
        let config = RetryPolicy::Default.to_config();
        assert_eq!(config.strategy, RetryStrategyType::Exponential);
        assert_eq!(config.max_attempts, 6);
        assert_eq!(config.base_delay, Duration::from_secs(2));
        assert_eq!(config.max_delay, Duration::from_secs(120));
    }

    #[test]
    fn test_retry_policy_to_config_aggressive() {
        let config = RetryPolicy::Aggressive.to_config();
        assert_eq!(config.strategy, RetryStrategyType::Exponential);
        assert_eq!(config.max_attempts, 10);
        assert_eq!(config.base_delay, Duration::from_millis(500));
        assert_eq!(config.max_delay, Duration::from_secs(30));
    }

    #[test]
    fn test_retry_policy_to_config_conservative() {
        let config = RetryPolicy::Conservative.to_config();
        assert_eq!(config.strategy, RetryStrategyType::Linear);
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.base_delay, Duration::from_secs(5));
        assert_eq!(config.max_delay, Duration::from_secs(60));
    }

    #[test]
    fn test_calculate_delay_immediate() {
        let config = RetryStrategyConfig {
            strategy: RetryStrategyType::Immediate,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            jitter: 0.0,
            max_attempts: 3,
        };

        let delay = calculate_delay(&config, 1);
        assert_eq!(delay, Duration::ZERO);

        let delay2 = calculate_delay(&config, 5);
        assert_eq!(delay2, Duration::ZERO);
    }

    #[test]
    fn test_calculate_delay_exponential() {
        let config = RetryStrategyConfig {
            strategy: RetryStrategyType::Exponential,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            jitter: 0.0,
            max_attempts: 10,
        };

        // Attempt 1: base_delay * 2^0 = 1s
        assert_eq!(calculate_delay(&config, 1), Duration::from_secs(1));

        // Attempt 2: base_delay * 2^1 = 2s
        assert_eq!(calculate_delay(&config, 2), Duration::from_secs(2));

        // Attempt 3: base_delay * 2^2 = 4s
        assert_eq!(calculate_delay(&config, 3), Duration::from_secs(4));

        // Attempt 10: should be capped at max_delay
        assert_eq!(calculate_delay(&config, 10), Duration::from_secs(60));
    }

    #[test]
    fn test_calculate_delay_linear() {
        let config = RetryStrategyConfig {
            strategy: RetryStrategyType::Linear,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            jitter: 0.0,
            max_attempts: 10,
        };

        // Attempt 1: base_delay * 1 = 1s
        assert_eq!(calculate_delay(&config, 1), Duration::from_secs(1));

        // Attempt 2: base_delay * 2 = 2s
        assert_eq!(calculate_delay(&config, 2), Duration::from_secs(2));

        // Attempt 5: base_delay * 5 = 5s
        assert_eq!(calculate_delay(&config, 5), Duration::from_secs(5));

        // Attempt 15: should be capped at max_delay
        assert_eq!(calculate_delay(&config, 15), Duration::from_secs(10));
    }

    #[test]
    fn test_calculate_delay_constant() {
        let config = RetryStrategyConfig {
            strategy: RetryStrategyType::Constant,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(10),
            jitter: 0.0,
            max_attempts: 10,
        };

        // All attempts should use base_delay
        assert_eq!(calculate_delay(&config, 1), Duration::from_secs(2));
        assert_eq!(calculate_delay(&config, 5), Duration::from_secs(2));
        assert_eq!(calculate_delay(&config, 10), Duration::from_secs(2));
    }

    #[test]
    fn test_calculate_delay_capped_at_max() {
        let config = RetryStrategyConfig {
            strategy: RetryStrategyType::Exponential,
            base_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(30),
            jitter: 0.0,
            max_attempts: 10,
        };

        // Exponential: 10 * 2^0 = 10s, 10 * 2^1 = 20s, 10 * 2^2 = 40s -> capped at 30s
        assert_eq!(calculate_delay(&config, 1), Duration::from_secs(10));
        assert_eq!(calculate_delay(&config, 2), Duration::from_secs(20));
        assert_eq!(calculate_delay(&config, 3), Duration::from_secs(30));
        assert_eq!(calculate_delay(&config, 10), Duration::from_secs(30));
    }

    #[test]
    fn test_config_for_error_uses_defaults() {
        let default_config = RetryStrategyConfig {
            strategy: RetryStrategyType::Exponential,
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: 0.5,
        };

        let result = config_for_error(&default_config, &None, ErrorClass::Retryable);
        assert_eq!(result.max_attempts, 5);

        let result = config_for_error(&default_config, &None, ErrorClass::Permanent);
        assert_eq!(result.max_attempts, 5);
    }

    #[test]
    fn test_config_for_error_uses_per_error() {
        let default_config = RetryStrategyConfig::default();

        let per_error = PerErrorConfig {
            retryable: Some(RetryStrategyConfig {
                strategy: RetryStrategyType::Immediate,
                max_attempts: 10,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                jitter: 0.0,
            }),
            ambiguous: None,
            permanent: None,
        };

        // Should use per-error config for retryable
        let result = config_for_error(
            &default_config,
            &Some(per_error.clone()),
            ErrorClass::Retryable,
        );
        assert_eq!(result.strategy, RetryStrategyType::Immediate);

        // Should fall back to default for ambiguous
        let result = config_for_error(&default_config, &Some(per_error), ErrorClass::Ambiguous);
        assert_eq!(result.strategy, RetryStrategyType::Exponential);
    }

    #[test]
    fn test_retry_strategy_config_serde() {
        let json = r#"{
            "strategy": "linear",
            "max_attempts": 3,
            "base_delay": "5s",
            "max_delay": "30s",
            "jitter": 0.2
        }"#;

        let config: RetryStrategyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.strategy, RetryStrategyType::Linear);
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.base_delay, Duration::from_secs(5));
        assert_eq!(config.max_delay, Duration::from_secs(30));
        assert!((config.jitter - 0.2).abs() < 0.001);
    }

    #[test]
    fn test_per_error_config_serde() {
        let json = r#"{
            "retryable": {
                "strategy": "immediate",
                "max_attempts": 10,
                "base_delay": "0s",
                "max_delay": "1s",
                "jitter": 0.0
            },
            "ambiguous": {
                "strategy": "exponential",
                "max_attempts": 5,
                "base_delay": "1s",
                "max_delay": "60s",
                "jitter": 0.3
            }
        }"#;

        let config: PerErrorConfig = serde_json::from_str(json).unwrap();
        assert!(config.retryable.is_some());
        assert!(config.ambiguous.is_some());
        assert!(config.permanent.is_none());

        assert_eq!(
            config.retryable.unwrap().strategy,
            RetryStrategyType::Immediate
        );
        assert_eq!(
            config.ambiguous.unwrap().strategy,
            RetryStrategyType::Exponential
        );
    }
}
