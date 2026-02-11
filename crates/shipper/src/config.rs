//! Configuration file support for Shipper (.shipper.toml)
//!
//! This module provides support for project-specific configuration via a
//! `.shipper.toml` file in the workspace root.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::types::{PublishPolicy, ReadinessConfig, RuntimeOptions, VerifyMode, deserialize_duration, ParallelConfig};

/// Nested policy configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PolicyConfig {
    /// Publishing policy: safe, balanced, or fast
    #[serde(default)]
    pub mode: PublishPolicy,
}

/// Nested verify configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerifyConfig {
    /// Verify mode: workspace, package, or none
    #[serde(default)]
    pub mode: VerifyMode,
}

/// Nested retry configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetryConfig {
    /// Max attempts per crate publish step
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Base backoff delay
    #[serde(deserialize_with = "deserialize_duration")]
    #[serde(default = "default_base_delay")]
    pub base_delay: Duration,

    /// Max backoff delay
    #[serde(deserialize_with = "deserialize_duration")]
    #[serde(default = "default_max_delay")]
    pub max_delay: Duration,
}

/// Nested output configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutputConfig {
    /// Number of output lines to capture for evidence
    #[serde(default = "default_output_lines")]
    pub lines: usize,
}

/// Nested lock configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LockConfig {
    /// Lock timeout duration
    #[serde(deserialize_with = "deserialize_duration")]
    #[serde(default = "default_lock_timeout")]
    pub timeout: Duration,
}

/// Nested flags configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlagsConfig {
    /// Allow publishing from a dirty git working tree
    #[serde(default)]
    pub allow_dirty: bool,

    /// Skip owners/permissions preflight
    #[serde(default)]
    pub skip_ownership_check: bool,

    /// Fail preflight if ownership checks fail
    #[serde(default)]
    pub strict_ownership: bool,
}

/// Configuration loaded from .shipper.toml
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShipperConfig {
    /// Publish policy configuration
    #[serde(default)]
    pub policy: PolicyConfig,

    /// Verify mode configuration
    #[serde(default)]
    pub verify: VerifyConfig,

    /// Readiness check configuration
    #[serde(default)]
    pub readiness: ReadinessConfig,

    /// Output configuration
    #[serde(default)]
    pub output: OutputConfig,

    /// Lock configuration
    #[serde(default)]
    pub lock: LockConfig,

    /// Retry configuration
    #[serde(default)]
    pub retry: RetryConfig,

    /// Flags configuration
    #[serde(default)]
    pub flags: FlagsConfig,

    /// Parallel publishing configuration
    #[serde(default)]
    pub parallel: ParallelConfig,

    /// Optional custom state directory
    #[serde(default)]
    pub state_dir: Option<PathBuf>,

    /// Optional custom registry configuration
    #[serde(default)]
    pub registry: Option<RegistryConfig>,
}

/// Registry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// Cargo registry name (e.g., crates-io)
    pub name: String,

    /// Base URL for registry web API (e.g., https://crates.io)
    pub api_base: String,
}

impl Default for ShipperConfig {
    fn default() -> Self {
        Self {
            policy: PolicyConfig {
                mode: PublishPolicy::default(),
            },
            verify: VerifyConfig {
                mode: VerifyMode::default(),
            },
            readiness: ReadinessConfig::default(),
            output: OutputConfig {
                lines: default_output_lines(),
            },
            lock: LockConfig {
                timeout: default_lock_timeout(),
            },
            retry: RetryConfig {
                max_attempts: default_max_attempts(),
                base_delay: default_base_delay(),
                max_delay: default_max_delay(),
            },
            flags: FlagsConfig {
                allow_dirty: false,
                skip_ownership_check: false,
                strict_ownership: false,
            },
            parallel: ParallelConfig::default(),
            state_dir: None,
            registry: None,
        }
    }
}

fn default_output_lines() -> usize {
    50
}

fn default_lock_timeout() -> Duration {
    Duration::from_secs(3600) // 1 hour
}

fn default_max_attempts() -> u32 {
    6
}

fn default_base_delay() -> Duration {
    Duration::from_secs(2)
}

fn default_max_delay() -> Duration {
    Duration::from_secs(120) // 2 minutes
}

impl ShipperConfig {
    /// Load configuration from workspace root by searching for .shipper.toml
    ///
    /// Returns `Ok(None)` if no config file exists.
    pub fn load_from_workspace(workspace_root: &Path) -> Result<Option<Self>> {
        let config_path = workspace_root.join(".shipper.toml");
        if !config_path.exists() {
            return Ok(None);
        }
        Self::load_from_file(&config_path).map(Some)
    }

    /// Load configuration from a specific file path
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: ShipperConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        // Validate output_lines
        if self.output.lines == 0 {
            bail!("output.lines must be greater than 0");
        }

        // Validate max_attempts
        if self.retry.max_attempts == 0 {
            bail!("retry.max_attempts must be greater than 0");
        }

        // Validate delays
        if self.retry.base_delay.is_zero() {
            bail!("retry.base_delay must be greater than 0");
        }

        if self.retry.max_delay < self.retry.base_delay {
            bail!("retry.max_delay must be greater than or equal to retry.base_delay");
        }

        // Validate lock_timeout
        if self.lock.timeout.is_zero() {
            bail!("lock.timeout must be greater than 0");
        }

        // Validate readiness config
        if self.readiness.max_total_wait.is_zero() {
            bail!("readiness.max_total_wait must be greater than 0");
        }

        if self.readiness.poll_interval.is_zero() {
            bail!("readiness.poll_interval must be greater than 0");
        }

        if self.readiness.jitter_factor < 0.0 || self.readiness.jitter_factor > 1.0 {
            bail!("readiness.jitter_factor must be between 0.0 and 1.0");
        }

        // Validate parallel config
        if self.parallel.max_concurrent == 0 {
            bail!("parallel.max_concurrent must be greater than 0");
        }

        if self.parallel.per_package_timeout.is_zero() {
            bail!("parallel.per_package_timeout must be greater than 0");
        }

        // Validate registry if present
        if let Some(ref registry) = self.registry {
            if registry.name.is_empty() {
                bail!("registry.name cannot be empty");
            }
            if registry.api_base.is_empty() {
                bail!("registry.api_base cannot be empty");
            }
        }

        Ok(())
    }

    /// Merge this configuration with CLI options.
    ///
    /// CLI options take precedence over config file values.
    /// This function applies config values only where CLI options weren't explicitly set.
    pub fn merge_with_cli_opts(&self, opts: RuntimeOptions) -> RuntimeOptions {
        RuntimeOptions {
            // CLI values always take precedence
            allow_dirty: opts.allow_dirty,
            skip_ownership_check: opts.skip_ownership_check,
            strict_ownership: opts.strict_ownership,
            no_verify: opts.no_verify,
            max_attempts: opts.max_attempts,
            base_delay: opts.base_delay,
            max_delay: opts.max_delay,
            verify_timeout: opts.verify_timeout,
            verify_poll_interval: opts.verify_poll_interval,
            state_dir: opts.state_dir,
            force_resume: opts.force_resume,
            force: opts.force,
            policy: opts.policy,
            verify_mode: opts.verify_mode,
            readiness: opts.readiness,
            output_lines: opts.output_lines,
            lock_timeout: opts.lock_timeout,
            parallel: opts.parallel,
        }
    }

    /// Generate a default configuration file content as TOML string
    pub fn default_toml_template() -> String {
        r#"# Shipper configuration file
# This file should be placed in your workspace root as .shipper.toml

[policy]
# Publishing policy: safe (verify+strict), balanced (verify when needed), or fast (no verify)
mode = "safe"

[verify]
# Verify mode: workspace (default, safest), package (per-crate), or none (no verify)
mode = "workspace"

[readiness]
# Enable readiness checks (wait for registry visibility after publish)
enabled = true
# Method for checking version visibility: api (fast), index (slower, more accurate), both (slowest, most reliable)
method = "api"
# Initial delay before first poll
initial_delay = "1s"
# Maximum delay between polls
max_delay = "60s"
# Maximum total time to wait for visibility
max_total_wait = "5m"
# Base poll interval
poll_interval = "2s"
# Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter)
jitter_factor = 0.5

[output]
# Number of output lines to capture for evidence
lines = 50

[lock]
# Lock timeout duration (locks older than this are considered stale)
timeout = "1h"

[retry]
# Max attempts per crate publish step
max_attempts = 6
# Base backoff delay
base_delay = "2s"
# Max backoff delay
max_delay = "2m"

[flags]
# Allow publishing from a dirty git working tree (not recommended)
allow_dirty = false
# Skip owners/permissions preflight (not recommended)
skip_ownership_check = false
# Fail preflight if ownership checks fail (recommended)
strict_ownership = false

[parallel]
# Enable parallel publishing (default: false for sequential)
enabled = false
# Maximum number of concurrent publish operations (default: 4)
max_concurrent = 4
# Timeout per package publish operation (default: 30 minutes)
per_package_timeout = "30m"

# Optional: Custom registry configuration
# [registry]
# name = "crates-io"
# api_base = "https://crates.io"
"#.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ShipperConfig::default();
        assert_eq!(config.policy.mode, PublishPolicy::Safe);
        assert_eq!(config.verify.mode, VerifyMode::Workspace);
        assert_eq!(config.output.lines, 50);
        assert_eq!(config.retry.max_attempts, 6);
        assert_eq!(config.flags.allow_dirty, false);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_output_lines() {
        let mut config = ShipperConfig::default();
        config.output.lines = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_max_attempts() {
        let mut config = ShipperConfig::default();
        config.retry.max_attempts = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_delays() {
        let mut config = ShipperConfig::default();
        config.retry.base_delay = Duration::ZERO;
        assert!(config.validate().is_err());

        config.retry.base_delay = Duration::from_secs(1);
        config.retry.max_delay = Duration::from_millis(500);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_jitter_factor() {
        let mut config = ShipperConfig::default();
        config.readiness.jitter_factor = 1.5;
        assert!(config.validate().is_err());

        config.readiness.jitter_factor = -0.1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_registry() {
        let mut config = ShipperConfig::default();
        config.registry = Some(RegistryConfig {
            name: String::new(),
            api_base: "https://crates.io".to_string(),
        });
        assert!(config.validate().is_err());

        config.registry = Some(RegistryConfig {
            name: "crates-io".to_string(),
            api_base: String::new(),
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_parse_toml_config() {
        let toml = r#"
[policy]
mode = "fast"

[verify]
mode = "none"

[readiness]
enabled = false
method = "api"
initial_delay = "1s"
max_delay = "60s"
max_total_wait = "5m"
poll_interval = "2s"
jitter_factor = 0.5

[output]
lines = 100

[lock]
timeout = "30m"

[retry]
max_attempts = 3
base_delay = "1s"
max_delay = "30s"

[flags]
allow_dirty = true
skip_ownership_check = true
"#;

        let config: ShipperConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.policy.mode, PublishPolicy::Fast);
        assert_eq!(config.verify.mode, VerifyMode::None);
        assert_eq!(config.readiness.enabled, false);
        assert_eq!(config.output.lines, 100);
        assert_eq!(config.lock.timeout, Duration::from_secs(1800));
        assert_eq!(config.retry.max_attempts, 3);
        assert_eq!(config.flags.allow_dirty, true);
        assert_eq!(config.flags.skip_ownership_check, true);
    }

    #[test]
    fn test_parse_toml_with_registry() {
        let toml = r#"
[registry]
name = "my-registry"
api_base = "https://my-registry.example.com"
"#;

        let config: ShipperConfig = toml::from_str(toml).unwrap();
        assert!(config.registry.is_some());
        let registry = config.registry.unwrap();
        assert_eq!(registry.name, "my-registry");
        assert_eq!(registry.api_base, "https://my-registry.example.com");
    }

    #[test]
    fn test_parse_toml_with_parallel() {
        let toml = r#"
[parallel]
enabled = true
max_concurrent = 8
per_package_timeout = "1h"
"#;

        let config: ShipperConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.parallel.enabled, true);
        assert_eq!(config.parallel.max_concurrent, 8);
        assert_eq!(config.parallel.per_package_timeout, Duration::from_secs(3600));
    }
}
