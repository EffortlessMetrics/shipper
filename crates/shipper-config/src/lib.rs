//! Configuration file support for Shipper (.shipper.toml)
//!
//! This module provides support for project-specific configuration via a
//! `.shipper.toml` file in the workspace root.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

pub use shipper_encrypt::EncryptionConfig;
use shipper_encrypt::EncryptionConfig as EncryptionSettings;
pub use shipper_types::{
    ParallelConfig, PublishPolicy, ReadinessConfig, ReadinessMethod, Registry, RuntimeOptions,
    VerifyMode, deserialize_duration, serialize_duration,
};
pub use shipper_webhook::WebhookConfig;

use shipper_retry::{PerErrorConfig, RetryPolicy, RetryStrategyType};
use shipper_storage::{CloudStorageConfig, StorageType};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Retry policy preset: default, aggressive, conservative, or custom
    #[serde(default)]
    pub policy: RetryPolicy,

    /// Max attempts per crate publish step (used when policy is custom or as fallback)
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Base backoff delay
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    #[serde(default = "default_base_delay")]
    pub base_delay: Duration,

    /// Max backoff delay
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    #[serde(default = "default_max_delay")]
    pub max_delay: Duration,

    /// Strategy type: immediate, exponential, linear, constant
    #[serde(default)]
    pub strategy: RetryStrategyType,

    /// Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter)
    #[serde(default = "default_jitter")]
    pub jitter: f64,

    /// Per-error-type retry configuration
    #[serde(default)]
    pub per_error: PerErrorConfig,
}

/// Nested output configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Number of output lines to capture for evidence
    #[serde(default = "default_output_lines")]
    pub lines: usize,
}

/// Nested lock configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockConfig {
    /// Lock timeout duration
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    #[serde(default = "default_lock_timeout")]
    pub timeout: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            policy: RetryPolicy::Default,
            max_attempts: default_max_attempts(),
            base_delay: default_base_delay(),
            max_delay: default_max_delay(),
            strategy: RetryStrategyType::Exponential,
            jitter: 0.5,
            per_error: PerErrorConfig::default(),
        }
    }
}

fn default_jitter() -> f64 {
    0.5
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            lines: default_output_lines(),
        }
    }
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            timeout: default_lock_timeout(),
        }
    }
}

/// Nested encryption configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EncryptionConfigInner {
    /// Enable encryption for state files
    #[serde(default)]
    pub enabled: bool,
    /// Passphrase for encryption/decryption (can also be set via SHIPPER_ENCRYPT_KEY env var)
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Environment variable to read passphrase from (default: SHIPPER_ENCRYPT_KEY)
    #[serde(default)]
    pub env_key: Option<String>,
}

/// Nested storage configuration for cloud storage backends
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageConfigInner {
    /// Storage type: file, s3, gcs, or azure
    #[serde(default)]
    pub storage_type: StorageType,
    /// Bucket/container name
    #[serde(default)]
    pub bucket: Option<String>,
    /// Region (for S3) or project ID (for GCS)
    #[serde(default)]
    pub region: Option<String>,
    /// Base path within the bucket
    #[serde(default)]
    pub base_path: Option<String>,
    /// Custom endpoint for S3-compatible services (MinIO, DigitalOcean Spaces, etc.)
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Access key ID
    #[serde(default)]
    pub access_key_id: Option<String>,
    /// Secret access key
    #[serde(default)]
    pub secret_access_key: Option<String>,
}

impl StorageConfigInner {
    /// Build CloudStorageConfig from this configuration
    ///
    /// Returns None if storage is not configured (i.e., using local file storage)
    pub fn to_cloud_config(&self) -> Option<CloudStorageConfig> {
        // Only build cloud config if bucket is specified
        let bucket = self.bucket.as_ref()?;

        let mut config = CloudStorageConfig::new(self.storage_type, bucket.clone());

        if let Some(ref region) = self.region {
            config.region = Some(region.clone());
        }
        if let Some(ref base_path) = self.base_path {
            config.base_path = base_path.clone();
        }
        if let Some(ref endpoint) = self.endpoint {
            config.endpoint = Some(endpoint.clone());
        }
        if let Some(ref access_key_id) = self.access_key_id {
            config.access_key_id = Some(access_key_id.clone());
        }
        if let Some(ref secret_access_key) = self.secret_access_key {
            config.secret_access_key = Some(secret_access_key.clone());
        }

        // Check for environment variable overrides
        config.access_key_id = config
            .access_key_id
            .clone()
            .or_else(|| std::env::var("SHIPPER_STORAGE_ACCESS_KEY_ID").ok());
        config.secret_access_key = config
            .secret_access_key
            .clone()
            .or_else(|| std::env::var("SHIPPER_STORAGE_SECRET_ACCESS_KEY").ok());
        config.region = config
            .region
            .clone()
            .or_else(|| std::env::var("SHIPPER_STORAGE_REGION").ok());

        Some(config)
    }

    /// Check if cloud storage is configured
    pub fn is_configured(&self) -> bool {
        self.bucket.is_some() && self.storage_type != StorageType::File
    }
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
    /// Schema version for the configuration file (e.g., `shipper.config.v1`)
    #[serde(default = "default_schema_version")]
    pub schema_version: String,

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

    /// Optional custom registry configuration (single registry)
    #[serde(default)]
    pub registry: Option<RegistryConfig>,

    /// Multiple registry configuration for multi-registry publishing
    #[serde(default)]
    pub registries: MultiRegistryConfig,

    /// Webhook configuration for publish notifications
    #[serde(default)]
    pub webhook: WebhookConfig,

    /// Encryption configuration for state files
    #[serde(default)]
    pub encryption: EncryptionConfigInner,

    /// Storage configuration for cloud storage backends
    #[serde(default)]
    pub storage: StorageConfigInner,
}

/// Registry configuration - supports both single registry and multiple registries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// Cargo registry name (e.g., crates-io)
    pub name: String,

    /// Base URL for registry web API (e.g., <https://crates.io>)
    pub api_base: String,

    /// Base URL for the sparse index (optional, derived from api_base if not set)
    #[serde(default)]
    pub index_base: Option<String>,

    /// Registry token (can also be set via environment variable)
    /// Supported formats:
    /// - "env:VAR_NAME" - read token from environment variable
    /// - "file:/path/to/token" - read token from file
    /// - Raw token string (not recommended for production)
    #[serde(default)]
    pub token: Option<String>,

    /// Whether this is the default registry (used when publishing to all registries)
    #[serde(default)]
    pub default: bool,
}

/// Multiple registry configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MultiRegistryConfig {
    /// List of registries to publish to
    #[serde(default)]
    pub registries: Vec<RegistryConfig>,

    /// Default registries to publish to if none specified (default: ["crates-io"])
    #[serde(default)]
    pub default_registries: Vec<String>,
}

impl MultiRegistryConfig {
    /// Get all registries, with crates-io as default if none configured
    pub fn get_registries(&self) -> Vec<RegistryConfig> {
        if self.registries.is_empty() {
            // Return default crates-io registry
            vec![RegistryConfig {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: Some("https://index.crates.io".to_string()),
                token: None,
                default: true,
            }]
        } else {
            self.registries.clone()
        }
    }

    /// Get the default registry (first one marked as default, or first one, or crates-io)
    pub fn get_default(&self) -> RegistryConfig {
        self.registries
            .iter()
            .find(|r| r.default)
            .or(self.registries.first())
            .cloned()
            .unwrap_or_else(|| RegistryConfig {
                name: "crates-io".to_string(),
                api_base: "https://crates.io".to_string(),
                index_base: Some("https://index.crates.io".to_string()),
                token: None,
                default: true,
            })
    }

    /// Find a registry by name
    pub fn find_by_name(&self, name: &str) -> Option<RegistryConfig> {
        self.registries.iter().find(|r| r.name == name).cloned()
    }
}

/// CLI overrides for merging with config file values.
///
/// `Option` fields mean "user did not pass this flag" when `None`.
/// `bool` fields mean "user explicitly enabled this" when `true`.
#[derive(Debug, Default)]
pub struct CliOverrides {
    pub policy: Option<PublishPolicy>,
    pub verify_mode: Option<VerifyMode>,
    pub max_attempts: Option<u32>,
    pub base_delay: Option<Duration>,
    pub max_delay: Option<Duration>,
    pub retry_strategy: Option<RetryStrategyType>,
    pub retry_jitter: Option<f64>,
    pub verify_timeout: Option<Duration>,
    pub verify_poll_interval: Option<Duration>,
    pub output_lines: Option<usize>,
    pub lock_timeout: Option<Duration>,
    pub state_dir: Option<PathBuf>,
    pub readiness_method: Option<ReadinessMethod>,
    pub readiness_timeout: Option<Duration>,
    pub readiness_poll: Option<Duration>,
    pub allow_dirty: bool,
    pub skip_ownership_check: bool,
    pub strict_ownership: bool,
    pub no_verify: bool,
    pub no_readiness: bool,
    pub force: bool,
    pub force_resume: bool,
    pub parallel_enabled: bool,
    pub max_concurrent: Option<usize>,
    pub per_package_timeout: Option<Duration>,
    pub webhook_url: Option<String>,
    pub webhook_secret: Option<String>,
    pub encrypt: bool,
    pub encrypt_passphrase: Option<String>,
    /// Target registries for multi-registry publishing (comma-separated list)
    pub registries: Option<Vec<String>>,
    /// Publish to all configured registries
    pub all_registries: bool,
    /// Optional package name to resume from
    pub resume_from: Option<String>,
}

impl Default for ShipperConfig {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
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
                policy: RetryPolicy::Default,
                max_attempts: default_max_attempts(),
                base_delay: default_base_delay(),
                max_delay: default_max_delay(),
                strategy: RetryStrategyType::Exponential,
                jitter: 0.5,
                per_error: PerErrorConfig::default(),
            },
            flags: FlagsConfig {
                allow_dirty: false,
                skip_ownership_check: false,
                strict_ownership: false,
            },
            parallel: ParallelConfig::default(),
            state_dir: None,
            registry: None,
            registries: MultiRegistryConfig::default(),
            webhook: WebhookConfig::default(),
            encryption: EncryptionConfigInner::default(),
            storage: StorageConfigInner::default(),
        }
    }
}

fn default_output_lines() -> usize {
    50
}

fn default_schema_version() -> String {
    "shipper.config.v1".to_string()
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

        // Validate schema version
        if let Err(e) = shipper_schema::validate_schema_version(
            &config.schema_version,
            "shipper.config.v1",
            "config",
        ) {
            bail!("{} in file: {}", e, path.display());
        }

        Ok(config)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        // Validate schema version format
        shipper_schema::parse_schema_version(&self.schema_version)
            .context("invalid schema_version format")?;

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

        // Validate jitter
        if self.retry.jitter < 0.0 || self.retry.jitter > 1.0 {
            bail!("retry.jitter must be between 0.0 and 1.0");
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

        // Validate multiple registries if present
        for reg in &self.registries.registries {
            if reg.name.is_empty() {
                bail!("registries[].name cannot be empty");
            }
            if reg.api_base.is_empty() {
                bail!("registries[].api_base cannot be empty");
            }
        }

        // Ensure only one default registry
        let default_count = self
            .registries
            .registries
            .iter()
            .filter(|r| r.default)
            .count();
        if default_count > 1 {
            bail!("only one registry can be marked as default");
        }

        Ok(())
    }

    /// Build `RuntimeOptions` by merging CLI overrides with config file values.
    ///
    /// For `Option` fields: CLI value takes precedence; falls back to config.
    /// For `bool` flags: `true` if either CLI or config enables it (OR).
    pub fn build_runtime_options(&self, cli: CliOverrides) -> RuntimeOptions {
        // Determine effective retry config based on policy
        let effective_retry = self.retry.policy.to_config();

        RuntimeOptions {
            allow_dirty: cli.allow_dirty || self.flags.allow_dirty,
            skip_ownership_check: cli.skip_ownership_check || self.flags.skip_ownership_check,
            strict_ownership: cli.strict_ownership || self.flags.strict_ownership,
            no_verify: cli.no_verify,
            max_attempts: cli
                .max_attempts
                .unwrap_or(if self.retry.policy == RetryPolicy::Custom {
                    self.retry.max_attempts
                } else {
                    effective_retry.max_attempts
                }),
            base_delay: cli
                .base_delay
                .unwrap_or(if self.retry.policy == RetryPolicy::Custom {
                    self.retry.base_delay
                } else {
                    effective_retry.base_delay
                }),
            max_delay: cli
                .max_delay
                .unwrap_or(if self.retry.policy == RetryPolicy::Custom {
                    self.retry.max_delay
                } else {
                    effective_retry.max_delay
                }),
            retry_strategy: cli.retry_strategy.unwrap_or(
                if self.retry.policy == RetryPolicy::Custom {
                    self.retry.strategy
                } else {
                    effective_retry.strategy
                },
            ),
            retry_jitter: cli
                .retry_jitter
                .unwrap_or(if self.retry.policy == RetryPolicy::Custom {
                    self.retry.jitter
                } else {
                    effective_retry.jitter
                }),
            retry_per_error: self.retry.per_error.clone(),
            verify_timeout: cli.verify_timeout.unwrap_or(Duration::from_secs(120)),
            verify_poll_interval: cli.verify_poll_interval.unwrap_or(Duration::from_secs(5)),
            state_dir: cli.state_dir.unwrap_or_else(|| {
                self.state_dir
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(".shipper"))
            }),
            force_resume: cli.force_resume,
            force: cli.force,
            lock_timeout: cli.lock_timeout.unwrap_or(self.lock.timeout),
            policy: cli.policy.unwrap_or(self.policy.mode),
            verify_mode: cli.verify_mode.unwrap_or(self.verify.mode),
            readiness: ReadinessConfig {
                enabled: !cli.no_readiness && self.readiness.enabled,
                method: cli.readiness_method.unwrap_or(self.readiness.method),
                initial_delay: self.readiness.initial_delay,
                max_delay: self.readiness.max_delay,
                max_total_wait: cli
                    .readiness_timeout
                    .unwrap_or(self.readiness.max_total_wait),
                poll_interval: cli.readiness_poll.unwrap_or(self.readiness.poll_interval),
                jitter_factor: self.readiness.jitter_factor,
                index_path: self.readiness.index_path.clone(),
                prefer_index: self.readiness.prefer_index,
            },
            output_lines: cli.output_lines.unwrap_or(self.output.lines),
            parallel: ParallelConfig {
                enabled: cli.parallel_enabled || self.parallel.enabled,
                max_concurrent: cli.max_concurrent.unwrap_or(self.parallel.max_concurrent),
                per_package_timeout: cli
                    .per_package_timeout
                    .unwrap_or(self.parallel.per_package_timeout),
            },
            webhook: {
                let mut cfg = self.webhook.clone();
                // CLI can override webhook settings
                if let Some(url) = cli.webhook_url {
                    cfg.url = url;
                }
                if let Some(secret) = cli.webhook_secret {
                    cfg.secret = Some(secret);
                }
                cfg
            },
            encryption: {
                let mut cfg = EncryptionSettings::default();
                // Enable encryption if CLI flag is set or config enables it
                if cli.encrypt || self.encryption.enabled {
                    cfg.enabled = true;
                }
                // CLI passphrase takes precedence over config
                if let Some(passphrase) = cli.encrypt_passphrase {
                    cfg.passphrase = Some(passphrase);
                } else if let Some(passphrase) = &self.encryption.passphrase {
                    cfg.passphrase = Some(passphrase.clone());
                }
                // Use env_key from config if set
                if let Some(ref env_key) = self.encryption.env_key {
                    cfg.env_var = Some(env_key.clone());
                } else if cfg.enabled && cfg.passphrase.is_none() {
                    // Default to SHIPPER_ENCRYPT_KEY if enabled but no passphrase
                    cfg.env_var = Some("SHIPPER_ENCRYPT_KEY".to_string());
                }
                cfg
            },
            registries: {
                // Determine target registries based on CLI overrides and config
                if cli.all_registries {
                    // Publish to all configured registries
                    self.registries
                        .get_registries()
                        .into_iter()
                        .map(|r| Registry {
                            name: r.name,
                            api_base: r.api_base,
                            index_base: r.index_base,
                        })
                        .collect()
                } else if let Some(ref reg_names) = cli.registries {
                    // Publish to specifically requested registries
                    reg_names
                        .iter()
                        .map(|name| {
                            // Try to find in config, otherwise use defaults
                            self.registries
                                .find_by_name(name)
                                .map(|r| Registry {
                                    name: r.name,
                                    api_base: r.api_base,
                                    index_base: r.index_base,
                                })
                                .unwrap_or_else(|| {
                                    // Default to crates-io if not found
                                    if name == "crates-io" {
                                        Registry::crates_io()
                                    } else {
                                        Registry {
                                            name: name.clone(),
                                            api_base: format!("https://{}.crates.io", name),
                                            index_base: None,
                                        }
                                    }
                                })
                        })
                        .collect()
                } else {
                    // Default: single registry from the plan
                    vec![]
                }
            },
            resume_from: cli.resume_from,
        }
    }

    /// Generate a default configuration file content as TOML string
    pub fn default_toml_template() -> String {
        r#"# Shipper configuration file
# This file should be placed in your workspace root as .shipper.toml

# Schema version for the configuration file
schema_version = "shipper.config.v1"

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
# Retry policy: default (balanced), aggressive, conservative, or custom
# - default: exponential backoff with 6 attempts, 2s base, 2m max
# - aggressive: exponential backoff with 10 attempts, 500ms base, 30s max
# - conservative: linear backoff with 3 attempts, 5s base, 60s max
# - custom: uses explicit strategy settings below
policy = "default"
# Max attempts per crate publish step (used when policy is custom)
max_attempts = 6
# Base backoff delay
base_delay = "2s"
# Max backoff delay
max_delay = "2m"
# Strategy type: immediate, exponential, linear, constant
strategy = "exponential"
# Jitter factor for randomized delays (0.0 = no jitter, 1.0 = full jitter)
jitter = 0.5

# Per-error-type retry configuration (optional)
# Uncomment and customize to override retry behavior for specific error types
# [retry.per_error.retryable]
# strategy = "immediate"
# max_attempts = 10
# base_delay = "0s"
# max_delay = "1s"
# jitter = 0.0

# [retry.per_error.ambiguous]
# strategy = "exponential"
# max_attempts = 5
# base_delay = "1s"
# max_delay = "60s"
# jitter = 0.3

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

# Optional: Webhook notifications for publish events
# [webhook]
# Enable webhook notifications (default: false - disabled)
# enabled = false
# URL to send POST requests to
# url = "https://your-webhook-endpoint.com/webhook"
# Optional secret for signing webhook payloads
# secret = "your-webhook-secret"
# Request timeout (default: 30s)
# timeout = "30s"
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
        assert!(!config.flags.allow_dirty);
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
        let mut config = ShipperConfig {
            schema_version: default_schema_version(),
            registry: Some(RegistryConfig {
                name: String::new(),
                api_base: "https://crates.io".to_string(),
                index_base: None,
                token: None,
                default: false,
            }),
            ..Default::default()
        };
        assert!(config.validate().is_err());

        config.registry = Some(RegistryConfig {
            name: "crates-io".to_string(),
            api_base: String::new(),
            index_base: None,
            token: None,
            default: false,
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
        assert!(!config.readiness.enabled);
        assert_eq!(config.output.lines, 100);
        assert_eq!(config.lock.timeout, Duration::from_secs(1800));
        assert_eq!(config.retry.max_attempts, 3);
        assert!(config.flags.allow_dirty);
        assert!(config.flags.skip_ownership_check);
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
        assert!(config.parallel.enabled);
        assert_eq!(config.parallel.max_concurrent, 8);
        assert_eq!(
            config.parallel.per_package_timeout,
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn test_parse_toml_with_partial_readiness_uses_defaults() {
        let toml = r#"
[readiness]
method = "both"
"#;

        let config: ShipperConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.readiness.method, ReadinessMethod::Both);
        assert!(config.readiness.enabled);
        assert_eq!(config.readiness.initial_delay, Duration::from_secs(1));
        assert_eq!(config.readiness.max_delay, Duration::from_secs(60));
        assert_eq!(config.readiness.max_total_wait, Duration::from_secs(300));
        assert_eq!(config.readiness.poll_interval, Duration::from_secs(2));
        assert_eq!(config.readiness.jitter_factor, 0.5);
    }

    #[test]
    fn test_parse_toml_with_partial_parallel_uses_defaults() {
        let toml = r#"
[parallel]
enabled = true
"#;

        let config: ShipperConfig = toml::from_str(toml).unwrap();
        assert!(config.parallel.enabled);
        assert_eq!(config.parallel.max_concurrent, 4);
        assert_eq!(
            config.parallel.per_package_timeout,
            Duration::from_secs(1800)
        );
    }

    #[test]
    fn test_parse_toml_with_partial_sections_remains_valid() {
        let toml = r#"
[readiness]
method = "both"

[parallel]
enabled = true
"#;

        let config: ShipperConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.output.lines, 50);
        assert_eq!(config.retry.max_attempts, 6);
        assert_eq!(config.lock.timeout, Duration::from_secs(3600));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_build_runtime_options_cli_overrides_config() {
        let config = ShipperConfig {
            schema_version: default_schema_version(),
            retry: RetryConfig {
                policy: RetryPolicy::Custom,
                max_attempts: 10,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(300),
                strategy: RetryStrategyType::Exponential,
                jitter: 0.5,
                per_error: PerErrorConfig::default(),
            },
            output: OutputConfig { lines: 100 },
            policy: PolicyConfig {
                mode: PublishPolicy::Balanced,
            },
            ..Default::default()
        };

        let cli = CliOverrides {
            max_attempts: Some(3),
            policy: Some(PublishPolicy::Fast),
            output_lines: Some(25),
            ..Default::default()
        };

        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.max_attempts, 3, "CLI max_attempts should win");
        assert_eq!(opts.policy, PublishPolicy::Fast, "CLI policy should win");
        assert_eq!(opts.output_lines, 25, "CLI output_lines should win");
    }

    #[test]
    fn test_build_runtime_options_config_used_when_cli_none() {
        let config = ShipperConfig {
            schema_version: default_schema_version(),
            retry: RetryConfig {
                policy: RetryPolicy::Custom,
                max_attempts: 10,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(300),
                strategy: RetryStrategyType::Exponential,
                jitter: 0.5,
                per_error: PerErrorConfig::default(),
            },
            output: OutputConfig { lines: 100 },
            policy: PolicyConfig {
                mode: PublishPolicy::Balanced,
            },
            verify: VerifyConfig {
                mode: VerifyMode::Package,
            },
            lock: LockConfig {
                timeout: Duration::from_secs(1800),
            },
            state_dir: Some(PathBuf::from("custom-state")),
            ..Default::default()
        };

        let cli = CliOverrides::default();

        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.max_attempts, 10, "config max_attempts should apply");
        assert_eq!(opts.base_delay, Duration::from_secs(5));
        assert_eq!(opts.max_delay, Duration::from_secs(300));
        assert_eq!(opts.output_lines, 100);
        assert_eq!(opts.policy, PublishPolicy::Balanced);
        assert_eq!(opts.verify_mode, VerifyMode::Package);
        assert_eq!(opts.lock_timeout, Duration::from_secs(1800));
        assert_eq!(opts.state_dir, PathBuf::from("custom-state"));
    }

    #[test]
    fn test_build_runtime_options_booleans_are_ored() {
        // Config sets allow_dirty, CLI doesn't
        let config = ShipperConfig {
            flags: FlagsConfig {
                allow_dirty: true,
                skip_ownership_check: false,
                strict_ownership: true,
            },
            ..Default::default()
        };

        let cli = CliOverrides {
            skip_ownership_check: true,
            ..Default::default()
        };

        let opts = config.build_runtime_options(cli);
        assert!(opts.allow_dirty, "config allow_dirty should apply");
        assert!(opts.skip_ownership_check, "CLI skip_ownership should apply");
        assert!(
            opts.strict_ownership,
            "config strict_ownership should apply"
        );
    }

    #[test]
    fn test_build_runtime_options_defaults_when_no_config() {
        let config = ShipperConfig::default();
        let cli = CliOverrides::default();

        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.max_attempts, 6);
        assert_eq!(opts.base_delay, Duration::from_secs(2));
        assert_eq!(opts.max_delay, Duration::from_secs(120));
        assert_eq!(opts.policy, PublishPolicy::Safe);
        assert_eq!(opts.verify_mode, VerifyMode::Workspace);
        assert_eq!(opts.output_lines, 50);
        assert_eq!(opts.state_dir, PathBuf::from(".shipper"));
        assert!(!opts.allow_dirty);
        assert!(!opts.no_verify);
        assert!(opts.readiness.enabled);
    }

    #[test]
    fn test_build_runtime_options_no_readiness_disables() {
        let config = ShipperConfig::default(); // readiness.enabled = true

        let cli = CliOverrides {
            no_readiness: true,
            ..Default::default()
        };

        let opts = config.build_runtime_options(cli);
        assert!(!opts.readiness.enabled);
    }

    #[test]
    fn test_build_runtime_options_parallel_merge() {
        let config = ShipperConfig {
            parallel: ParallelConfig {
                enabled: true,
                max_concurrent: 8,
                per_package_timeout: Duration::from_secs(7200),
            },
            ..Default::default()
        };

        // CLI doesn't set parallel, but config enables it
        let cli = CliOverrides::default();
        let opts = config.build_runtime_options(cli);
        assert!(opts.parallel.enabled);
        assert_eq!(opts.parallel.max_concurrent, 8);
        assert_eq!(opts.parallel.per_package_timeout, Duration::from_secs(7200));

        // CLI overrides max_concurrent
        let cli2 = CliOverrides {
            max_concurrent: Some(2),
            ..Default::default()
        };
        let opts2 = config.build_runtime_options(cli2);
        assert!(opts2.parallel.enabled); // from config
        assert_eq!(opts2.parallel.max_concurrent, 2); // from CLI
    }

    mod snapshot_tests {
        use super::*;

        #[test]
        fn snapshot_default_config() {
            let config = ShipperConfig::default();
            insta::assert_yaml_snapshot!("default_config", config);
        }

        #[test]
        fn snapshot_config_all_fields_set() {
            let config = ShipperConfig {
                schema_version: "shipper.config.v1".to_string(),
                policy: PolicyConfig {
                    mode: PublishPolicy::Fast,
                },
                verify: VerifyConfig {
                    mode: VerifyMode::None,
                },
                readiness: ReadinessConfig {
                    enabled: false,
                    method: ReadinessMethod::Both,
                    initial_delay: Duration::from_secs(5),
                    max_delay: Duration::from_secs(120),
                    max_total_wait: Duration::from_secs(600),
                    poll_interval: Duration::from_secs(10),
                    jitter_factor: 0.3,
                    index_path: Some(std::path::PathBuf::from("/tmp/index")),
                    prefer_index: true,
                },
                output: OutputConfig { lines: 200 },
                lock: LockConfig {
                    timeout: Duration::from_secs(7200),
                },
                retry: RetryConfig {
                    policy: RetryPolicy::Aggressive,
                    max_attempts: 10,
                    base_delay: Duration::from_millis(500),
                    max_delay: Duration::from_secs(30),
                    strategy: RetryStrategyType::Linear,
                    jitter: 0.1,
                    per_error: PerErrorConfig::default(),
                },
                flags: FlagsConfig {
                    allow_dirty: true,
                    skip_ownership_check: true,
                    strict_ownership: true,
                },
                parallel: ParallelConfig {
                    enabled: true,
                    max_concurrent: 8,
                    per_package_timeout: Duration::from_secs(3600),
                },
                state_dir: Some(std::path::PathBuf::from("/custom/state")),
                registry: Some(RegistryConfig {
                    name: "my-registry".to_string(),
                    api_base: "https://my-registry.example.com".to_string(),
                    index_base: Some("https://index.my-registry.example.com".to_string()),
                    token: None,
                    default: true,
                }),
                registries: MultiRegistryConfig::default(),
                webhook: WebhookConfig::default(),
                encryption: EncryptionConfigInner {
                    enabled: true,
                    passphrase: None,
                    env_key: Some("MY_ENCRYPT_KEY".to_string()),
                },
                storage: StorageConfigInner {
                    storage_type: StorageType::default(),
                    bucket: Some("my-bucket".to_string()),
                    region: Some("us-east-1".to_string()),
                    base_path: Some("releases/".to_string()),
                    endpoint: None,
                    access_key_id: None,
                    secret_access_key: None,
                },
            };
            insta::assert_yaml_snapshot!("config_all_fields", config);
        }

        #[test]
        fn snapshot_validation_error_zero_output_lines() {
            let mut config = ShipperConfig::default();
            config.output.lines = 0;
            let err = config.validate().unwrap_err();
            insta::assert_yaml_snapshot!("validation_error_zero_output_lines", err.to_string());
        }

        #[test]
        fn snapshot_validation_error_zero_max_attempts() {
            let mut config = ShipperConfig::default();
            config.retry.max_attempts = 0;
            let err = config.validate().unwrap_err();
            insta::assert_yaml_snapshot!("validation_error_zero_max_attempts", err.to_string());
        }

        #[test]
        fn snapshot_validation_error_zero_base_delay() {
            let mut config = ShipperConfig::default();
            config.retry.base_delay = Duration::ZERO;
            let err = config.validate().unwrap_err();
            insta::assert_yaml_snapshot!("validation_error_zero_base_delay", err.to_string());
        }

        #[test]
        fn snapshot_validation_error_max_delay_less_than_base() {
            let mut config = ShipperConfig::default();
            config.retry.base_delay = Duration::from_secs(10);
            config.retry.max_delay = Duration::from_secs(5);
            let err = config.validate().unwrap_err();
            insta::assert_yaml_snapshot!("validation_error_max_delay_lt_base", err.to_string());
        }

        #[test]
        fn snapshot_validation_error_jitter_out_of_range() {
            let mut config = ShipperConfig::default();
            config.retry.jitter = 1.5;
            let err = config.validate().unwrap_err();
            insta::assert_yaml_snapshot!("validation_error_jitter_out_of_range", err.to_string());
        }

        #[test]
        fn snapshot_validation_error_empty_registry_name() {
            let config = ShipperConfig {
                registry: Some(RegistryConfig {
                    name: String::new(),
                    api_base: "https://crates.io".to_string(),
                    index_base: None,
                    token: None,
                    default: false,
                }),
                ..ShipperConfig::default()
            };
            let err = config.validate().unwrap_err();
            insta::assert_yaml_snapshot!("validation_error_empty_registry_name", err.to_string());
        }

        #[test]
        fn snapshot_toml_roundtrip() {
            let toml_input = r#"
schema_version = "shipper.config.v1"

[policy]
mode = "balanced"

[verify]
mode = "package"

[readiness]
enabled = true
method = "index"
initial_delay = "2s"
max_delay = "30s"
max_total_wait = "3m"
poll_interval = "5s"
jitter_factor = 0.25

[output]
lines = 75

[lock]
timeout = "45m"

[retry]
policy = "conservative"
max_attempts = 3
base_delay = "5s"
max_delay = "1m"
strategy = "linear"
jitter = 0.2

[flags]
allow_dirty = false
skip_ownership_check = false
strict_ownership = true

[parallel]
enabled = true
max_concurrent = 2
per_package_timeout = "15m"
"#;

            let parsed: ShipperConfig = toml::from_str(toml_input).unwrap();
            let re_serialized = toml::to_string_pretty(&parsed).unwrap();
            let re_parsed: ShipperConfig = toml::from_str(&re_serialized).unwrap();
            insta::assert_yaml_snapshot!("toml_roundtrip_parsed", re_parsed);
        }

        #[test]
        fn snapshot_default_toml_template() {
            let template = ShipperConfig::default_toml_template();
            insta::assert_snapshot!("default_toml_template", template);
        }
    }

    #[cfg(test)]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_policy() -> impl Strategy<Value = PublishPolicy> {
            prop_oneof![
                Just(PublishPolicy::Safe),
                Just(PublishPolicy::Balanced),
                Just(PublishPolicy::Fast),
            ]
        }

        fn arb_verify_mode() -> impl Strategy<Value = VerifyMode> {
            prop_oneof![
                Just(VerifyMode::Workspace),
                Just(VerifyMode::Package),
                Just(VerifyMode::None),
            ]
        }

        fn arb_retry_policy() -> impl Strategy<Value = RetryPolicy> {
            prop_oneof![
                Just(RetryPolicy::Default),
                Just(RetryPolicy::Aggressive),
                Just(RetryPolicy::Conservative),
                Just(RetryPolicy::Custom),
            ]
        }

        fn arb_retry_strategy() -> impl Strategy<Value = RetryStrategyType> {
            prop_oneof![
                Just(RetryStrategyType::Immediate),
                Just(RetryStrategyType::Exponential),
                Just(RetryStrategyType::Linear),
                Just(RetryStrategyType::Constant),
            ]
        }

        fn arb_readiness_method() -> impl Strategy<Value = ReadinessMethod> {
            prop_oneof![
                Just(ReadinessMethod::Api),
                Just(ReadinessMethod::Index),
                Just(ReadinessMethod::Both),
            ]
        }

        /// Generate a valid `ShipperConfig` that always passes `validate()`.
        fn arb_valid_config() -> impl Strategy<Value = ShipperConfig> {
            let enums = (
                arb_policy(),
                arb_verify_mode(),
                arb_retry_policy(),
                arb_retry_strategy(),
                arb_readiness_method(),
            );
            let retry_nums = (
                1u32..100,    // max_attempts
                1u64..3600,   // base_delay secs
                0u64..3600,   // extra secs added to base for max_delay
                0.0f64..=1.0, // jitter
            );
            let config_nums = (
                1usize..500, // output lines
                1u64..7200,  // lock_timeout secs
                1usize..32,  // max_concurrent
                1u64..7200,  // per_package_timeout secs
            );
            let booleans = (
                any::<bool>(), // allow_dirty
                any::<bool>(), // skip_ownership
                any::<bool>(), // strict_ownership
                any::<bool>(), // readiness enabled
                any::<bool>(), // parallel enabled
            );
            let readiness_nums = (
                1u64..600,    // initial_delay secs
                1u64..600,    // max_delay secs
                1u64..600,    // max_total_wait secs
                1u64..60,     // poll_interval secs
                0.0f64..=1.0, // jitter_factor
            );

            (enums, retry_nums, config_nums, booleans, readiness_nums).prop_map(
                |(
                    (policy, verify, retry_policy, retry_strategy, readiness_method),
                    (max_attempts, base_delay, extra_delay, jitter),
                    (output_lines, lock_timeout, max_concurrent, per_package_timeout),
                    (
                        allow_dirty,
                        skip_ownership,
                        strict_ownership,
                        readiness_enabled,
                        parallel_enabled,
                    ),
                    (r_initial, r_max_delay, r_max_total, r_poll, r_jitter),
                )| {
                    ShipperConfig {
                        schema_version: default_schema_version(),
                        policy: PolicyConfig { mode: policy },
                        verify: VerifyConfig { mode: verify },
                        readiness: ReadinessConfig {
                            enabled: readiness_enabled,
                            method: readiness_method,
                            initial_delay: Duration::from_secs(r_initial),
                            max_delay: Duration::from_secs(r_max_delay),
                            max_total_wait: Duration::from_secs(r_max_total),
                            poll_interval: Duration::from_secs(r_poll),
                            jitter_factor: r_jitter,
                            index_path: None,
                            prefer_index: false,
                        },
                        output: OutputConfig {
                            lines: output_lines,
                        },
                        lock: LockConfig {
                            timeout: Duration::from_secs(lock_timeout),
                        },
                        retry: RetryConfig {
                            policy: retry_policy,
                            max_attempts,
                            base_delay: Duration::from_secs(base_delay),
                            max_delay: Duration::from_secs(base_delay + extra_delay),
                            strategy: retry_strategy,
                            jitter,
                            per_error: PerErrorConfig::default(),
                        },
                        flags: FlagsConfig {
                            allow_dirty,
                            skip_ownership_check: skip_ownership,
                            strict_ownership,
                        },
                        parallel: ParallelConfig {
                            enabled: parallel_enabled,
                            max_concurrent,
                            per_package_timeout: Duration::from_secs(per_package_timeout),
                        },
                        state_dir: None,
                        registry: None,
                        registries: MultiRegistryConfig::default(),
                        webhook: WebhookConfig::default(),
                        encryption: EncryptionConfigInner::default(),
                        storage: StorageConfigInner::default(),
                    }
                },
            )
        }

        proptest! {
            #[test]
            fn cli_max_attempts_overrides_custom_retry_settings(
                cfg_max_attempts in 1u32..300,
                cli_max_attempts in proptest::option::of(1u32..300),
                max_delay in 1u64..10_000,
                base_delay in 1u64..5_000,
                no_readiness in any::<bool>(),
                allow_dirty in any::<bool>(),
                skip_ownership in any::<bool>(),
                strict_ownership in any::<bool>(),
            ) {
                let config = ShipperConfig {
                    schema_version: default_schema_version(),
                    retry: RetryConfig {
                        policy: RetryPolicy::Custom,
                        max_attempts: cfg_max_attempts,
                        base_delay: Duration::from_millis(base_delay),
                        max_delay: Duration::from_millis(max_delay.max(base_delay)),
                        strategy: RetryStrategyType::Exponential,
                        jitter: 0.5,
                        per_error: PerErrorConfig::default(),
                    },
                    flags: FlagsConfig {
                        allow_dirty,
                        skip_ownership_check: skip_ownership,
                        strict_ownership,
                    },
                    readiness: ReadinessConfig { enabled: !no_readiness, ..Default::default() },
                    parallel: ParallelConfig {
                        enabled: true,
                        max_concurrent: 4,
                        per_package_timeout: Duration::from_secs(600),
                    },
                    ..Default::default()
                };

                let cli = CliOverrides {
                    max_attempts: cli_max_attempts,
                    output_lines: Some(73),
                    no_readiness,
                    allow_dirty,
                    skip_ownership_check: skip_ownership,
                    strict_ownership,
                    ..Default::default()
                };

                let opts = config.build_runtime_options(cli);

                assert_eq!(
                    opts.max_attempts,
                    cli_max_attempts.unwrap_or(cfg_max_attempts)
                );
                assert_eq!(opts.allow_dirty, allow_dirty);
                assert_eq!(opts.skip_ownership_check, skip_ownership);
                assert_eq!(opts.strict_ownership, strict_ownership);
                assert_eq!(opts.readiness.enabled, !no_readiness);
                assert_eq!(opts.parallel.max_concurrent, 4);
            }

            /// Any valid config serializes to TOML and deserializes back identically.
            #[test]
            fn toml_roundtrip_preserves_config(config in arb_valid_config()) {
                let toml1 = toml::to_string_pretty(&config)
                    .expect("first serialize must succeed");
                let parsed: ShipperConfig = toml::from_str(&toml1)
                    .expect("deserialize of serialized config must succeed");
                let toml2 = toml::to_string_pretty(&parsed)
                    .expect("second serialize must succeed");
                prop_assert_eq!(toml1, toml2);
            }

            /// Validation always succeeds for default config, regardless of seed.
            #[test]
            fn default_config_always_validates(_seed in any::<u64>()) {
                let config = ShipperConfig::default();
                prop_assert!(config.validate().is_ok());
            }

            /// Every generated valid config passes validation.
            #[test]
            fn generated_valid_config_passes_validation(config in arb_valid_config()) {
                prop_assert!(config.validate().is_ok());
            }

            /// Any valid config serializes to parseable TOML.
            #[test]
            fn valid_config_serializes_to_valid_toml(config in arb_valid_config()) {
                let toml_str = toml::to_string_pretty(&config)
                    .expect("serialize must succeed");
                let reparsed: Result<ShipperConfig, _> = toml::from_str(&toml_str);
                prop_assert!(reparsed.is_ok(), "re-parse failed: {:?}", reparsed.err());
            }

            /// build_runtime_options with default (empty) CLI overrides preserves
            /// config-sourced values (merge idempotency for the config side).
            #[test]
            fn merge_with_empty_overrides_preserves_config(config in arb_valid_config()) {
                let cli = CliOverrides::default();
                let opts = config.build_runtime_options(cli);

                prop_assert_eq!(opts.allow_dirty, config.flags.allow_dirty);
                prop_assert_eq!(opts.skip_ownership_check, config.flags.skip_ownership_check);
                prop_assert_eq!(opts.strict_ownership, config.flags.strict_ownership);
                prop_assert_eq!(opts.output_lines, config.output.lines);
                prop_assert_eq!(opts.lock_timeout, config.lock.timeout);
                prop_assert_eq!(opts.policy, config.policy.mode);
                prop_assert_eq!(opts.verify_mode, config.verify.mode);
                prop_assert_eq!(opts.readiness.enabled, config.readiness.enabled);
                prop_assert_eq!(opts.readiness.method, config.readiness.method);
                prop_assert_eq!(opts.parallel.enabled, config.parallel.enabled);
                prop_assert_eq!(opts.parallel.max_concurrent, config.parallel.max_concurrent);
                prop_assert_eq!(
                    opts.parallel.per_package_timeout,
                    config.parallel.per_package_timeout
                );
            }
        }
    }
}
