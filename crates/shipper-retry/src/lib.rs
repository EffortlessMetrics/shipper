//! Retry strategies and backoff policies for distributed systems.
//!
//! This crate provides configurable retry strategies with support for:
//! - Multiple backoff strategies (immediate, exponential, linear, constant)
//! - Jitter for avoiding thundering herd problems
//! - Per-error-type configuration
//! - Predefined policies for common use cases
//!
//! # Example
//!
//! ```
//! use shipper_retry::{RetryPolicy, RetryStrategyConfig, calculate_delay};
//! use std::time::Duration;
//!
//! // Use a predefined policy
//! let config = RetryPolicy::Default.to_config();
//! let delay = calculate_delay(&config, 2);
//! println!("Retry after: {:?}", delay);
//!
//! // Custom configuration
//! let custom = RetryStrategyConfig {
//!     max_attempts: 5,
//!     base_delay: Duration::from_secs(1),
//!     max_delay: Duration::from_secs(30),
//!     ..Default::default()
//! };
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

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
    #[serde(default = "default_base_delay")]
    #[serde(with = "humantime_serde")]
    pub base_delay: Duration,
    /// Maximum delay cap for backoff.
    #[serde(default = "default_max_delay")]
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    /// Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter).
    #[serde(default = "default_jitter")]
    pub jitter: f64,
}

fn default_base_delay() -> Duration {
    Duration::from_secs(2)
}

fn default_max_delay() -> Duration {
    Duration::from_secs(120)
}

fn default_jitter() -> f64 {
    0.5
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

/// Error classification for retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// Error is transient and should be retried
    #[default]
    Retryable,
    /// Error outcome is unknown (may have succeeded)
    Ambiguous,
    /// Error is permanent and should not be retried
    Permanent,
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
///
/// # Arguments
///
/// * `config` - The retry strategy configuration
/// * `attempt` - The current attempt number (1-indexed)
///
/// # Returns
///
/// The duration to wait before the next retry attempt.
///
/// # Example
///
/// ```
/// use shipper_retry::{RetryStrategyConfig, RetryStrategyType, calculate_delay};
/// use std::time::Duration;
///
/// let config = RetryStrategyConfig {
///     strategy: RetryStrategyType::Exponential,
///     base_delay: Duration::from_secs(1),
///     max_delay: Duration::from_secs(60),
///     jitter: 0.0,
///     max_attempts: 10,
/// };
///
/// let delay = calculate_delay(&config, 1);
/// assert_eq!(delay, Duration::from_secs(1));
///
/// let delay = calculate_delay(&config, 2);
/// assert_eq!(delay, Duration::from_secs(2));
/// ```
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
    use rand::Rng;

    // Generate a random factor between (1 - jitter) and (1 + jitter)
    let jitter_range = 2.0 * jitter;
    let mut rng = rand::thread_rng();
    let random_value: f64 = rng.r#gen();
    let random_factor = 1.0 - jitter + (random_value * jitter_range);
    let millis = (delay.as_millis() as f64 * random_factor).round() as u64;
    Duration::from_millis(millis)
}

/// Get the retry configuration for a specific error class.
/// Falls back to the default config if no per-error config is specified.
///
/// # Arguments
///
/// * `default_config` - The default retry configuration
/// * `per_error_config` - Optional per-error-type configuration
/// * `error_class` - The classification of the error
///
/// # Returns
///
/// The appropriate retry configuration for the error class.
pub fn config_for_error(
    default_config: &RetryStrategyConfig,
    per_error_config: Option<&PerErrorConfig>,
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

/// A retry executor that runs a fallible operation with configured retry behavior.
pub struct RetryExecutor {
    config: RetryStrategyConfig,
}

impl RetryExecutor {
    /// Create a new retry executor with the given configuration.
    pub fn new(config: RetryStrategyConfig) -> Self {
        Self { config }
    }

    /// Create a retry executor from a predefined policy.
    pub fn from_policy(policy: RetryPolicy) -> Self {
        Self::new(policy.to_config())
    }

    /// Execute a fallible operation with retry behavior.
    ///
    /// The operation receives the current attempt number (starting at 1).
    /// Return `Ok(T)` on success, `Err(E)` on failure.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use shipper_retry::{RetryExecutor, RetryPolicy};
    ///
    /// let executor = RetryExecutor::from_policy(RetryPolicy::Default);
    /// let result = executor.run(|attempt| {
    ///     // Your fallible operation here
    ///     if attempt < 3 {
    ///         Err("transient error")
    ///     } else {
    ///         Ok("success")
    ///     }
    /// });
    /// ```
    pub fn run<T, E, F>(&self, mut operation: F) -> Result<T, E>
    where
        F: FnMut(u32) -> Result<T, E>,
    {
        let mut attempt = 1;

        loop {
            match operation(attempt) {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if attempt >= self.config.max_attempts {
                        return Err(e);
                    }

                    let delay = calculate_delay(&self.config, attempt);
                    std::thread::sleep(delay);
                    attempt += 1;
                }
            }
        }
    }

    /// Execute a fallible operation with retry behavior and custom error classification.
    ///
    /// The operation returns a tuple of (result, should_retry).
    /// This allows the operation to indicate whether an error is retryable.
    pub fn run_with_classification<T, E, F>(&self, mut operation: F) -> Result<T, E>
    where
        F: FnMut(u32) -> Result<(T, bool), E>,
    {
        let mut attempt = 1;

        loop {
            match operation(attempt) {
                Ok((result, _)) => return Ok(result),
                Err(e) => {
                    if attempt >= self.config.max_attempts {
                        return Err(e);
                    }

                    let delay = calculate_delay(&self.config, attempt);
                    std::thread::sleep(delay);
                    attempt += 1;
                }
            }
        }
    }
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

        assert_eq!(calculate_delay(&config, 1), Duration::ZERO);
        assert_eq!(calculate_delay(&config, 5), Duration::ZERO);
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

        assert_eq!(calculate_delay(&config, 1), Duration::from_secs(1));
        assert_eq!(calculate_delay(&config, 2), Duration::from_secs(2));
        assert_eq!(calculate_delay(&config, 5), Duration::from_secs(5));
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

        let result = config_for_error(&default_config, None, ErrorClass::Retryable);
        assert_eq!(result.max_attempts, 5);

        let result = config_for_error(&default_config, None, ErrorClass::Permanent);
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
        let result = config_for_error(&default_config, Some(&per_error), ErrorClass::Retryable);
        assert_eq!(result.strategy, RetryStrategyType::Immediate);

        // Should fall back to default for ambiguous
        let result = config_for_error(&default_config, Some(&per_error), ErrorClass::Ambiguous);
        assert_eq!(result.strategy, RetryStrategyType::Exponential);
    }

    #[test]
    fn test_retry_executor_success_on_first_try() {
        let executor = RetryExecutor::from_policy(RetryPolicy::Aggressive);
        let result = executor.run(|_attempt| Ok::<_, &str>("success"));
        assert_eq!(result, Ok("success"));
    }

    #[test]
    fn test_retry_executor_success_after_retries() {
        let executor = RetryExecutor::new(RetryStrategyConfig {
            strategy: RetryStrategyType::Immediate,
            max_attempts: 5,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter: 0.0,
        });

        let mut attempts = 0;
        let result = executor.run(|attempt| {
            attempts = attempt;
            if attempt < 3 {
                Err("transient error")
            } else {
                Ok("success")
            }
        });

        assert_eq!(result, Ok("success"));
        assert_eq!(attempts, 3);
    }

    #[test]
    fn test_retry_executor_fails_after_max_attempts() {
        let executor = RetryExecutor::new(RetryStrategyConfig {
            strategy: RetryStrategyType::Immediate,
            max_attempts: 3,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter: 0.0,
        });

        let result = executor.run(|_attempt| Err::<&str, _>("permanent error"));
        assert_eq!(result, Err("permanent error"));
    }

    #[test]
    fn test_jitter_applied_correctly() {
        let config = RetryStrategyConfig {
            strategy: RetryStrategyType::Constant,
            base_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(60),
            jitter: 0.5,
            max_attempts: 10,
        };

        // With jitter of 0.5, delay should be between 5s and 15s
        for _ in 0..100 {
            let delay = calculate_delay(&config, 1);
            assert!(delay >= Duration::from_millis(5000));
            assert!(delay <= Duration::from_millis(15000));
        }
    }
}