//! Retry contract values.
//!
//! These are the pure configuration values that describe *how* a retry should
//! be shaped — strategy, attempt ceiling, delay bounds, jitter, and per-error
//! overrides. They carry no retry behavior: delay calculation, jitter
//! randomness, sleeping, and the execution loop live in `shipper-retry`, which
//! consumes the definitions here and re-exports them for compatibility.
//!
//! Keeping the values in the contract crate is what lets `shipper-types`
//! describe [`crate::RuntimeOptions`]'s retry settings without depending on the
//! crate that performs the retries.
//!
//! Note that `ErrorClass` is deliberately *not* here. There are currently two
//! of them — [`crate::ErrorClass`] and `shipper_retry::ErrorClass` — with the
//! same three variants and the same `snake_case` wire form. Nothing moved by
//! this module references either, so collapsing them is a separate change with
//! its own compatibility question; see #261's note on duplicate serde
//! ownership.

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

/// Configuration for a retry strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryStrategyConfig {
    /// Strategy type for calculating delay between retries.
    #[serde(default)]
    pub strategy: RetryStrategyType,
    /// Maximum number of retry attempts.
    ///
    /// For compatibility, an omitted serialized value remains 0 while
    /// [`Self::default`] uses 6. Shipper rejects configured per-error policies
    /// before execution, so their omitted value does not become a retry budget.
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

/// Default base delay for backoff, used as the `serde` default for
/// [`RetryStrategyConfig::base_delay`]. A configuration file that omits the
/// field deserializes to this, so it is a persisted contract value.
fn default_base_delay() -> Duration {
    Duration::from_secs(2)
}

/// Default backoff ceiling, used as the `serde` default for
/// [`RetryStrategyConfig::max_delay`].
fn default_max_delay() -> Duration {
    Duration::from_mins(2)
}

/// Default jitter factor, used as the `serde` default for
/// [`RetryStrategyConfig::jitter`].
fn default_jitter() -> f64 {
    0.5
}

impl Default for RetryStrategyConfig {
    fn default() -> Self {
        Self {
            strategy: RetryStrategyType::Exponential,
            max_attempts: 6,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_mins(2),
            jitter: 0.5,
        }
    }
}

/// Reserved per-error-type retry configuration.
///
/// Shipper's config validation and public engine entry points reject any
/// configured class. The shape remains available for diagnostics and migration;
/// an omitted or empty configuration retains the global retry policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerErrorConfig {
    /// Retry configuration for retryable errors (e.g., network issues, rate limiting).
    #[serde(default, rename = "retryable")]
    pub retryable: Option<RetryStrategyConfig>,
    /// Retry configuration for ambiguous errors (e.g., unknown if publish succeeded).
    #[serde(default, rename = "ambiguous")]
    pub ambiguous: Option<RetryStrategyConfig>,
    /// Retry configuration for permanent errors (e.g., authentication failure).
    /// This does not enable automatic retries of permanent errors in Shipper.
    #[serde(default, rename = "permanent")]
    pub permanent: Option<RetryStrategyConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_type_defaults_to_exponential() {
        assert_eq!(RetryStrategyType::default(), RetryStrategyType::Exponential);
    }

    #[test]
    fn strategy_type_serializes_as_snake_case() {
        // Persisted `.shipper.toml` and state files depend on these exact
        // strings; the derive alone does not pin them.
        for (variant, expected) in [
            (RetryStrategyType::Immediate, "\"immediate\""),
            (RetryStrategyType::Exponential, "\"exponential\""),
            (RetryStrategyType::Linear, "\"linear\""),
            (RetryStrategyType::Constant, "\"constant\""),
        ] {
            assert_eq!(
                serde_json::to_string(&variant).expect("serialize"),
                expected
            );
        }
    }

    #[test]
    fn default_config_matches_its_serde_defaults() {
        // `Default` and the `serde` field defaults are written independently,
        // so a config that omits every field must equal `Default::default()`.
        // If they drift, an omitted field silently means something else.
        let from_empty: RetryStrategyConfig =
            serde_json::from_str("{}").expect("deserialize empty");
        let from_default = RetryStrategyConfig::default();

        assert_eq!(from_empty.strategy, from_default.strategy);
        assert_eq!(from_empty.base_delay, from_default.base_delay);
        assert_eq!(from_empty.max_delay, from_default.max_delay);
        assert_eq!(from_empty.jitter, from_default.jitter);

        // Pin the literal values too. Comparing the two default paths against
        // each other only proves they agree — a change applied to both would
        // pass while silently altering every configuration that omits a field.
        assert_eq!(from_default.strategy, RetryStrategyType::Exponential);
        assert_eq!(from_default.base_delay, Duration::from_secs(2));
        assert_eq!(from_default.max_delay, Duration::from_mins(2));
        assert_eq!(from_default.jitter, 0.5);
        // `max_attempts` is the deliberate exception: its `serde` default is
        // `u32::default()` (0) while `Default` uses 6. Pin the difference so a
        // change to either is visible rather than accidental.
        assert_eq!(from_empty.max_attempts, 0);
        assert_eq!(from_default.max_attempts, 6);
    }

    #[test]
    fn delay_fields_accept_humantime_strings_and_round_trip() {
        let config: RetryStrategyConfig =
            serde_json::from_str(r#"{"base_delay":"5s","max_delay":"3m"}"#).expect("deserialize");
        assert_eq!(config.base_delay, Duration::from_secs(5));
        assert_eq!(config.max_delay, Duration::from_mins(3));

        let json = serde_json::to_string(&config).expect("serialize");
        let restored: RetryStrategyConfig = serde_json::from_str(&json).expect("round trip");
        assert_eq!(restored.base_delay, config.base_delay);
        assert_eq!(restored.max_delay, config.max_delay);
    }

    #[test]
    fn per_error_config_defaults_every_class_to_none() {
        let config = PerErrorConfig::default();
        assert!(config.retryable.is_none());
        assert!(config.ambiguous.is_none());
        assert!(config.permanent.is_none());

        let from_empty: PerErrorConfig = serde_json::from_str("{}").expect("deserialize empty");
        assert!(from_empty.retryable.is_none());
        assert!(from_empty.ambiguous.is_none());
        assert!(from_empty.permanent.is_none());
    }

    #[test]
    fn per_error_config_keys_are_the_error_class_names() {
        let config: PerErrorConfig = serde_json::from_str(
            r#"{"retryable":{"max_attempts":9},"ambiguous":{"max_attempts":2},"permanent":{"max_attempts":1}}"#,
        )
        .expect("deserialize");

        assert_eq!(config.retryable.expect("retryable").max_attempts, 9);
        assert_eq!(config.ambiguous.expect("ambiguous").max_attempts, 2);
        assert_eq!(config.permanent.expect("permanent").max_attempts, 1);
    }
}
