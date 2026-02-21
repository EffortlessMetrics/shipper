//! Configuration file handling for shipper.
//!
//! This crate provides configuration loading from `.shipper.toml` files
//! with support for merging with CLI arguments and defaults.
//!
//! # Example
//!
//! ```
//! use shipper_config::{Config, load_config};
//! use std::path::Path;
//!
//! // Load config from a directory (looks for .shipper.toml)
//! let config = load_config(Path::new(".")).expect("load config");
//!
//! // Access configuration values
//! if let Some(registry) = config.registry() {
//!     println!("Registry: {}", registry);
//! }
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use shipper_types::{PublishPolicy, VerifyMode};

/// Default configuration file name
pub const CONFIG_FILE: &str = ".shipper.toml";

/// Get the config file path for a directory
pub fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

/// Complete shipper configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Registry configuration
    #[serde(default)]
    registry: RegistryConfig,
    /// Retry configuration
    #[serde(default)]
    retry: RetryConfig,
    /// Readiness check configuration
    #[serde(default)]
    readiness: ReadinessConfig,
    /// Publish behavior configuration
    #[serde(default)]
    publish: PublishConfig,
    /// Preflight check configuration
    #[serde(default)]
    preflight: PreflightConfig,
}

impl Config {
    /// Create a new config with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the registry name
    pub fn registry(&self) -> Option<&str> {
        self.registry.name.as_deref()
    }

    /// Get the registry API URL
    pub fn registry_url(&self) -> Option<&str> {
        self.registry.url.as_deref()
    }

    /// Get the maximum retry attempts
    pub fn max_retries(&self) -> u32 {
        self.retry.max_attempts
    }

    /// Get the initial retry delay
    pub fn retry_delay(&self) -> Duration {
        Duration::from_secs(self.retry.initial_delay_secs)
    }

    /// Get the maximum retry delay
    pub fn max_retry_delay(&self) -> Duration {
        Duration::from_secs(self.retry.max_delay_secs)
    }

    /// Get the retry multiplier
    pub fn retry_multiplier(&self) -> f64 {
        self.retry.multiplier
    }

    /// Get the readiness check timeout
    pub fn readiness_timeout(&self) -> Duration {
        Duration::from_secs(self.readiness.timeout_secs)
    }

    /// Get the readiness check interval
    pub fn readiness_interval(&self) -> Duration {
        Duration::from_secs(self.readiness.interval_secs)
    }

    /// Get the publish policy
    pub fn publish_policy(&self) -> PublishPolicy {
        self.publish.policy
    }

    /// Get the verify mode
    pub fn verify_mode(&self) -> VerifyMode {
        self.publish.verify
    }

    /// Whether to allow dirty git state
    pub fn allow_dirty(&self) -> bool {
        self.preflight.allow_dirty
    }

    /// Whether to skip dry-run verification
    pub fn skip_dry_run(&self) -> bool {
        self.preflight.skip_dry_run
    }

    /// Merge this config with another (other takes precedence)
    pub fn merge(&self, other: &Config) -> Config {
        Config {
            registry: RegistryConfig {
                name: other.registry.name.as_ref().or(self.registry.name.as_ref()).cloned(),
                url: other.registry.url.as_ref().or(self.registry.url.as_ref()).cloned(),
            },
            retry: RetryConfig {
                max_attempts: if other.retry.max_attempts != 3 {
                    other.retry.max_attempts
                } else {
                    self.retry.max_attempts
                },
                initial_delay_secs: if other.retry.initial_delay_secs != 1 {
                    other.retry.initial_delay_secs
                } else {
                    self.retry.initial_delay_secs
                },
                max_delay_secs: if other.retry.max_delay_secs != 60 {
                    other.retry.max_delay_secs
                } else {
                    self.retry.max_delay_secs
                },
                multiplier: if other.retry.multiplier != 2.0 {
                    other.retry.multiplier
                } else {
                    self.retry.multiplier
                },
            },
            readiness: ReadinessConfig {
                timeout_secs: if other.readiness.timeout_secs != 300 {
                    other.readiness.timeout_secs
                } else {
                    self.readiness.timeout_secs
                },
                interval_secs: if other.readiness.interval_secs != 5 {
                    other.readiness.interval_secs
                } else {
                    self.readiness.interval_secs
                },
            },
            publish: PublishConfig {
                policy: other.publish.policy,
                verify: other.publish.verify,
            },
            preflight: PreflightConfig {
                allow_dirty: other.preflight.allow_dirty || self.preflight.allow_dirty,
                skip_dry_run: other.preflight.skip_dry_run || self.preflight.skip_dry_run,
            },
        }
    }
}

/// Registry configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// Registry name (e.g., "crates-io", "my-registry")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Registry API URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Initial delay in seconds
    #[serde(default = "default_initial_delay")]
    pub initial_delay_secs: u64,
    /// Maximum delay in seconds
    #[serde(default = "default_max_delay")]
    pub max_delay_secs: u64,
    /// Backoff multiplier
    #[serde(default = "default_multiplier")]
    pub multiplier: f64,
}

fn default_max_attempts() -> u32 { 3 }
fn default_initial_delay() -> u64 { 1 }
fn default_max_delay() -> u64 { 60 }
fn default_multiplier() -> f64 { 2.0 }

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            initial_delay_secs: default_initial_delay(),
            max_delay_secs: default_max_delay(),
            multiplier: default_multiplier(),
        }
    }
}

/// Readiness check configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessConfig {
    /// Timeout in seconds for readiness checks
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Interval between readiness checks
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
}

fn default_timeout() -> u64 { 300 }
fn default_interval() -> u64 { 5 }

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout(),
            interval_secs: default_interval(),
        }
    }
}

/// Publish behavior configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishConfig {
    /// Publish policy preset
    #[serde(default)]
    pub policy: PublishPolicy,
    /// Verification mode
    #[serde(default)]
    pub verify: VerifyMode,
}

/// Preflight check configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreflightConfig {
    /// Allow publishing with dirty git state
    #[serde(default)]
    pub allow_dirty: bool,
    /// Skip dry-run verification
    #[serde(default)]
    pub skip_dry_run: bool,
}

/// Load configuration from a directory
pub fn load_config(dir: &Path) -> Result<Config> {
    let path = config_path(dir);
    if !path.exists() {
        return Ok(Config::default());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;

    let config: Config = toml::from_str(&content)
        .with_context(|| format!("failed to parse config file: {}", path.display()))?;

    Ok(config)
}

/// Load configuration from a specific file path
pub fn load_config_from_file(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;

    let config: Config = toml::from_str(&content)
        .with_context(|| format!("failed to parse config file: {}", path.display()))?;

    Ok(config)
}

/// Save configuration to a file
pub fn save_config(dir: &Path, config: &Config) -> Result<()> {
    let path = config_path(dir);

    let content = toml::to_string_pretty(config)
        .context("failed to serialize config to TOML")?;

    std::fs::write(&path, content)
        .with_context(|| format!("failed to write config file: {}", path.display()))?;

    Ok(())
}

/// Find configuration file by walking up the directory tree
pub fn find_config(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir;

    loop {
        let config_file = current.join(CONFIG_FILE);
        if config_file.exists() {
            return Some(config_file);
        }

        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config() {
        let config = Config::new();
        assert!(config.registry().is_none());
        assert_eq!(config.max_retries(), 3);
        assert_eq!(config.retry_delay(), Duration::from_secs(1));
        assert_eq!(config.readiness_timeout(), Duration::from_secs(300));
    }

    #[test]
    fn load_missing_config_returns_default() {
        let td = tempdir().expect("tempdir");
        let config = load_config(td.path()).expect("load");
        assert!(config.registry().is_none());
    }

    #[test]
    fn save_and_load_config() {
        let td = tempdir().expect("tempdir");

        let mut config = Config::new();
        config.registry.name = Some("my-registry".to_string());
        config.retry.max_attempts = 5;

        save_config(td.path(), &config).expect("save");

        let loaded = load_config(td.path()).expect("load");
        assert_eq!(loaded.registry(), Some("my-registry"));
        assert_eq!(loaded.max_retries(), 5);
    }

    #[test]
    fn load_config_from_toml() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CONFIG_FILE);

        let content = r#"
[registry]
name = "custom-registry"
url = "https://custom.registry.io"

[retry]
max_attempts = 10
initial_delay_secs = 2

[readiness]
timeout_secs = 600
interval_secs = 10

[publish]
policy = "fast"
verify = "none"

[preflight]
allow_dirty = true
"#;
        std::fs::write(&path, content).expect("write");

        let config = load_config(td.path()).expect("load");

        assert_eq!(config.registry(), Some("custom-registry"));
        assert_eq!(config.registry_url(), Some("https://custom.registry.io"));
        assert_eq!(config.max_retries(), 10);
        assert_eq!(config.retry_delay(), Duration::from_secs(2));
        assert_eq!(config.readiness_timeout(), Duration::from_secs(600));
        assert_eq!(config.readiness_interval(), Duration::from_secs(10));
        assert_eq!(config.publish_policy(), PublishPolicy::Fast);
        assert_eq!(config.verify_mode(), VerifyMode::None);
        assert!(config.allow_dirty());
    }

    #[test]
    fn merge_configs() {
        let mut base = Config::new();
        base.registry.name = Some("base-registry".to_string());
        base.retry.max_attempts = 3;

        let mut override_config = Config::new();
        override_config.registry.name = Some("override-registry".to_string());
        override_config.retry.max_attempts = 5;

        let merged = base.merge(&override_config);

        assert_eq!(merged.registry(), Some("override-registry"));
        assert_eq!(merged.max_retries(), 5);
    }

    #[test]
    fn find_config_walks_up() {
        let td = tempdir().expect("tempdir");

        // Create nested directories
        let nested = td.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).expect("create dirs");

        // Write config at root
        let config_path = td.path().join(CONFIG_FILE);
        std::fs::write(&config_path, "[registry]\nname = 'test'").expect("write");

        // Find from nested directory
        let found = find_config(&nested);
        assert!(found.is_some());
        assert_eq!(found.unwrap(), config_path);
    }

    #[test]
    fn find_config_returns_none_if_not_found() {
        let td = tempdir().expect("tempdir");
        let nested = td.path().join("a").join("b");
        std::fs::create_dir_all(&nested).expect("create dirs");

        let found = find_config(&nested);
        assert!(found.is_none());
    }

    #[test]
    fn config_path_helper() {
        let dir = PathBuf::from("/project");
        assert_eq!(config_path(&dir), PathBuf::from("/project/.shipper.toml"));
    }

    #[test]
    fn partial_config_uses_defaults() {
        let td = tempdir().expect("tempdir");
        let path = td.path().join(CONFIG_FILE);

        // Only specify registry name, rest should be defaults
        let content = r#"
[registry]
name = "partial"
"#;
        std::fs::write(&path, content).expect("write");

        let config = load_config(td.path()).expect("load");

        assert_eq!(config.registry(), Some("partial"));
        assert_eq!(config.max_retries(), 3); // default
        assert_eq!(config.readiness_timeout(), Duration::from_secs(300)); // default
    }
}