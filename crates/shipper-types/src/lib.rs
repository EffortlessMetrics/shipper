//! # Types
//!
//! Core domain types for Shipper, including specs, plans, options, receipts, and errors.
//!
//! This module defines the fundamental data structures used throughout Shipper:
//! - [`ReleaseSpec`] - Input specification for a publish operation
//! - [`ReleasePlan`] - Deterministic, SHA256-identified publish plan  
//! - [`RuntimeOptions`] - All runtime configuration options
//! - [`Receipt`] - Audit receipt with evidence for each published crate
//! - [`PreflightReport`] - Preflight assessment with finishability verdict
//! - [`PublishPolicy`] - Policy presets for safety vs. speed tradeoffs
//!
//! ## Serialization
//!
//! Most types implement `Serialize` and `Deserialize` from `serde` for
//! persistence to disk. Durations are serialized as milliseconds for
//! cross-platform compatibility.
//!
//! ## Stability
//!
//! These types are considered stable unless otherwise noted. Breaking
//! changes will be documented in the changelog.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DurationMilliSeconds, serde_as};

use shipper_encrypt::EncryptionConfig as EncryptionSettings;
use shipper_webhook::WebhookConfig;

/// Deserialize a Duration from either a string (human-readable) or u64 (milliseconds)
pub fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DurationHelper {
        String(String),
        U64(u64),
    }

    match DurationHelper::deserialize(deserializer)? {
        DurationHelper::String(s) => humantime::parse_duration(&s)
            .map_err(|e| serde::de::Error::custom(format!("invalid duration: {}", e))),
        DurationHelper::U64(ms) => Ok(Duration::from_millis(ms)),
    }
}

/// Serialize a Duration as milliseconds (u64) so it roundtrips with deserialize_duration
pub fn serialize_duration<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(duration.as_millis() as u64)
}

/// Represents a Cargo registry for publishing crates.
///
/// A registry is identified by its name (used with `cargo publish --registry <name>`)
/// and its API/base URLs. The default registry is crates.io, which can be created
/// using [`Registry::crates_io()`].
///
/// # Example
///
/// ```rust
/// use shipper::types::Registry;
///
/// // Use crates.io (default)
/// let crates_io = Registry::crates_io();
///
/// // Custom registry
/// let my_registry = Registry {
///     name: "my-registry".to_string(),
///     api_base: "https://my-registry.example.com".to_string(),
///     index_base: Some("https://index.my-registry.example.com".to_string()),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    /// Cargo registry name (for `cargo publish --registry <name>`). For crates.io this is typically `crates-io`.
    pub name: String,
    /// Base URL for registry web API, e.g. `https://crates.io`.
    pub api_base: String,
    /// Base URL for the sparse index, e.g. `https://index.crates.io`.
    /// If not specified, will be derived from the API base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_base: Option<String>,
}

impl Registry {
    /// Creates a new [`Registry`] configured for crates.io.
    ///
    /// This is the default registry used by Cargo and is the most common
    /// target for publishing Rust crates.
    ///
    /// # Returns
    ///
    /// A [`Registry`] with:
    /// - name: `"crates-io"`
    /// - api_base: `"https://crates.io"`
    /// - index_base: `Some("https://index.crates.io")`
    ///
    /// # Example
    ///
    /// ```rust
    /// use shipper::types::Registry;
    ///
    /// let registry = Registry::crates_io();
    /// assert_eq!(registry.name, "crates-io");
    /// assert_eq!(registry.api_base, "https://crates.io");
    /// ```
    pub fn crates_io() -> Self {
        Self {
            name: "crates-io".to_string(),
            api_base: "https://crates.io".to_string(),
            index_base: Some("https://index.crates.io".to_string()),
        }
    }

    /// Get the index base URL, deriving it from the API base if not explicitly set.
    /// Strips the `sparse+` prefix if present (used by Cargo's sparse index config).
    pub fn get_index_base(&self) -> String {
        if let Some(index_base) = &self.index_base {
            index_base
                .strip_prefix("sparse+")
                .unwrap_or(index_base)
                .to_string()
        } else {
            // Default: derive from API base (e.g., https://crates.io -> https://index.crates.io)
            self.api_base
                .replace("https://", "https://index.")
                .replace("http://", "http://index.")
        }
    }
}

/// Input specification for a crate publish operation.
///
/// This is the primary entry point for configuring a Shipper publish operation.
/// It defines what to publish, where to publish it, and which packages to include.
///
/// # Example
///
/// ```no_run
/// use std::path::PathBuf;
/// use shipper::types::{ReleaseSpec, Registry};
///
/// let spec = ReleaseSpec {
///     manifest_path: PathBuf::from("Cargo.toml"),
///     registry: Registry::crates_io(),
///     selected_packages: None, // Publish all packages
/// };
///
/// // Or with specific packages
/// let specific_spec = ReleaseSpec {
///     manifest_path: PathBuf::from("Cargo.toml"),
///     registry: Registry::crates_io(),
///     selected_packages: Some(vec!["my-crate".to_string()]),
/// };
/// ```
///
/// # Fields
///
/// - `manifest_path`: Path to the workspace's `Cargo.toml`
/// - `registry`: Target [`Registry`] for publishing
/// - `selected_packages`: Optional list of package names to publish (None = all)
#[derive(Debug, Clone)]
pub struct ReleaseSpec {
    /// Path to the workspace's `Cargo.toml` manifest.
    pub manifest_path: PathBuf,
    /// Target registry for publishing.
    pub registry: Registry,
    /// Optional list of package names to publish. If `None`, all publishable
    /// packages in the workspace will be published.
    pub selected_packages: Option<Vec<String>>,
}

/// Policy presets that control the balance between safety and speed in publishing.
///
/// These policies determine which preflight checks and readiness verifications
/// are performed during the publish process. Choosing a more conservative policy
/// increases reliability at the cost of longer execution time.
///
/// # Example
///
/// ```rust
/// use shipper::types::PublishPolicy;
///
/// // Default: maximum safety
/// let safe = PublishPolicy::Safe;
///
/// // Balanced: skip some checks for known-good scenarios
/// let balanced = PublishPolicy::Balanced;
///
/// // Fast: minimal verification, maximum risk
/// let fast = PublishPolicy::Fast;
/// ```
///
/// # Variants
///
/// - [`PublishPolicy::Safe`] - Full preflight verification and readiness checks (default)
/// - [`PublishPolicy::Balanced`] - Verify only when needed for experienced users
/// - [`PublishPolicy::Fast`] - Skip all verification, assume the user knows what they're doing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishPolicy {
    /// Verify + strict checks (default)
    ///
    /// This is the default policy. It performs:
    /// - Full preflight verification (git cleanliness, dry-run, version existence)
    /// - Readiness checks after publishing
    /// - Ownership verification if applicable
    #[default]
    Safe,
    /// Verify only when needed
    ///
    /// Skips some checks that are redundant in well-tested workflows.
    /// Suitable for CI/CD pipelines with established release processes.
    Balanced,
    /// No verify; explicit risk
    ///
    /// Disables all verification. Use only when you understand the risks
    /// and have verified the publish process manually. Faster but dangerous.
    Fast,
}

/// Controls when and how `cargo verify` is run before publishing.
///
/// Verification compiles the crate to ensure it builds correctly before
/// attempting to publish. This adds safety but increases publish time.
///
/// # Example
///
/// ```rust
/// use shipper::types::VerifyMode;
///
/// // Verify the entire workspace at once (most efficient)
/// let workspace = VerifyMode::Workspace;
///
/// // Verify each crate individually (more thorough)
/// let package = VerifyMode::Package;
///
/// // Skip verification entirely
/// let none = VerifyMode::None;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// Default, safest - run workspace dry-run
    ///
    /// Runs `cargo verify` on the entire workspace once. This is the
    /// default and most efficient option.
    #[default]
    Workspace,
    /// Per-crate verify
    ///
    /// Runs `cargo verify` for each crate individually before publishing.
    /// More thorough but slower than workspace mode.
    Package,
    /// No verify
    ///
    /// Skips verification entirely. Use with caution.
    None,
}

/// Method for verifying crate visibility after publishing.
///
/// After a crate is published, Shipper can verify it becomes visible on
/// the registry before proceeding. This catches issues like propagation
/// delays or rejected publishes that Cargo might not report immediately.
///
/// # Example
///
/// ```rust
/// use shipper::types::ReadinessMethod;
///
/// // Fast: check the registry HTTP API
/// let api = ReadinessMethod::Api;
///
/// // Accurate: check the sparse index directly
/// let index = ReadinessMethod::Index;
///
/// // Reliable: check both (slowest)
/// let both = ReadinessMethod::Both;
/// ```
///
/// # Performance
///
/// - `Api`: ~1-2 requests per crate (fastest)
/// - `Index`: ~10-50 requests per crate (slower, most accurate)
/// - `Both`: Combines both methods (slowest, most reliable)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessMethod {
    /// Check crates.io HTTP API (default, fast)
    ///
    /// Makes HTTP requests to the registry's API to check if the
    /// version is visible. Fast but may not catch all edge cases.
    #[default]
    Api,
    /// Check sparse index (slower, more accurate)
    ///
    /// Downloads and checks the sparse index for the crate.
    /// More accurate than API but requires more requests.
    Index,
    /// Check both (slowest, most reliable)
    ///
    /// Uses both API and index methods, only passing if both
    /// confirm visibility. Most reliable but slowest.
    Both,
}

/// Configuration for readiness verification after publishing.
///
/// Readiness verification confirms that a published crate is visible on
/// the registry before Shipper considers the publish successful. This
/// catches propagation delays and failed publishes early.
///
/// # Example
///
/// ```rust
/// use std::time::Duration;
/// use shipper::types::{ReadinessConfig, ReadinessMethod};
///
/// // Default configuration
/// let config = ReadinessConfig::default();
///
/// // Custom configuration
/// let custom = ReadinessConfig {
///     enabled: true,
///     method: ReadinessMethod::Both,
///     initial_delay: Duration::from_secs(2),
///     max_delay: Duration::from_secs(120),
///     max_total_wait: Duration::from_secs(600), // 10 minutes
///     poll_interval: Duration::from_secs(5),
///     jitter_factor: 0.3,
///     index_path: None,
///     prefer_index: false,
/// };
/// ```
///
/// # Defaults
///
/// - `enabled`: `true`
/// - `method`: [`ReadinessMethod::Api`]
/// - `initial_delay`: 1 second
/// - `max_delay`: 60 seconds
/// - `max_total_wait`: 300 seconds (5 minutes)
/// - `poll_interval`: 2 seconds
/// - `jitter_factor`: 0.5 (±50%)
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadinessConfig {
    /// Enable readiness checks
    ///
    /// When disabled, Shipper will not verify crate visibility after
    /// publishing. This speeds up publishing but may miss failures.
    pub enabled: bool,
    /// Method for checking version visibility
    pub method: ReadinessMethod,
    /// Initial delay before first poll
    ///
    /// Most registries need a few seconds to propagate new versions.
    /// This delay allows the initial propagation to complete before
    /// starting to poll.
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub initial_delay: Duration,
    /// Maximum delay between polls (capped)
    ///
    /// The poll interval starts at the initial_delay value and increases
    /// exponentially up to this maximum.
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub max_delay: Duration,
    /// Maximum total time to wait for visibility
    ///
    /// If the crate is not visible within this time, the publish is
    /// considered failed. This prevents waiting indefinitely.
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub max_total_wait: Duration,
    /// Base poll interval
    ///
    /// The interval between readiness checks. This is the starting
    /// interval before jitter and exponential backoff are applied.
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub poll_interval: Duration,
    /// Jitter factor (±50% means 0.5)
    ///
    /// Adds randomness to poll intervals to reduce thundering herd
    /// when many clients are checking simultaneously. A value of 0.5
    /// means the actual interval varies by ±50%.
    pub jitter_factor: f64,
    /// Custom index path for testing (optional)
    ///
    /// When set, uses this local path instead of downloading from
    /// the remote index. Useful for testing with mock registries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_path: Option<PathBuf>,
    /// Use index as primary method when Both is selected
    ///
    /// When [`ReadinessMethod::Both`] is used, this determines which
    /// method is checked first. If `true`, the index is checked first.
    #[serde(default)]
    pub prefer_index: bool,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            max_total_wait: Duration::from_secs(300), // 5 minutes
            poll_interval: Duration::from_secs(2),
            jitter_factor: 0.5,
            index_path: None,
            prefer_index: false,
        }
    }
}

/// Configuration for parallel publishing.
///
/// Parallel publishing allows independent crates in a workspace to be
/// published concurrently, significantly reducing total publish time
/// for large workspaces with many independent crates.
///
/// # Example
///
/// ```rust
/// use std::time::Duration;
/// use shipper::types::ParallelConfig;
///
/// // Default: sequential publishing
/// let sequential = ParallelConfig::default();
///
/// // Enable parallel publishing
/// let parallel = ParallelConfig {
///     enabled: true,
///     max_concurrent: 4,
///     per_package_timeout: Duration::from_secs(1800), // 30 minutes
/// };
/// ```
///
/// # How It Works
///
/// Shipper analyzes the dependency graph and groups crates into "levels".
/// Crates at the same level have no dependencies on each other and can
/// be published in parallel. Crates at higher levels must wait for all
/// crates at lower levels to complete.
///
/// # Defaults
///
/// - `enabled`: `false` (sequential by default)
/// - `max_concurrent`: 4
/// - `per_package_timeout`: 1800 seconds (30 minutes)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParallelConfig {
    /// Enable parallel publishing (default: false for sequential)
    ///
    /// When disabled (the default), crates are published one at a time
    /// in dependency order. When enabled, independent crates are
    /// published concurrently.
    pub enabled: bool,
    /// Maximum number of concurrent publish operations (default: 4)
    ///
    /// The maximum number of crates that can be publishing simultaneously.
    /// This limits resource usage and API rate limiting impact.
    pub max_concurrent: usize,
    /// Timeout per package publish operation (default: 30 minutes)
    ///
    /// If a single package publish takes longer than this duration,
    /// it will be aborted and retried. This prevents a slow publish
    /// from blocking the entire operation.
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub per_package_timeout: Duration,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_concurrent: 4,
            per_package_timeout: Duration::from_secs(1800), // 30 minutes
        }
    }
}

/// Runtime configuration options for a Shipper publish operation.
///
/// This struct contains all the tunable parameters that control how
/// Shipper executes a publish operation, including retry behavior,
/// verification settings, and output preferences.
///
/// # Example
///
/// ```no_run
/// use std::path::PathBuf;
/// use shipper::types::{RuntimeOptions, PublishPolicy, ParallelConfig};
///
/// let options = RuntimeOptions {
///     allow_dirty: false,
///     skip_ownership_check: false,
///     strict_ownership: true,
///     no_verify: false,
///     max_attempts: 3,
///     base_delay: std::time::Duration::from_secs(1),
///     max_delay: std::time::Duration::from_secs(60),
///     retry_strategy: shipper::retry::RetryStrategyType::Exponential,
///     retry_jitter: 0.3,
///     retry_per_error: shipper::retry::PerErrorConfig::default(),
///     verify_timeout: std::time::Duration::from_secs(600),
///     verify_poll_interval: std::time::Duration::from_secs(10),
///     state_dir: PathBuf::from(".shipper"),
///     force_resume: false,
///     policy: PublishPolicy::Safe,
///     verify_mode: shipper::types::VerifyMode::Workspace,
///     readiness: shipper::types::ReadinessConfig::default(),
///     output_lines: 1000,
///     force: false,
///     lock_timeout: std::time::Duration::from_secs(3600),
///     parallel: ParallelConfig::default(),
///     webhook: shipper::webhook::WebhookConfig::default(),
///     encryption: shipper::encryption::EncryptionConfig::default(),
///     registries: vec![],
/// };
/// ```
#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub allow_dirty: bool,
    pub skip_ownership_check: bool,
    pub strict_ownership: bool,
    pub no_verify: bool,
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Retry strategy type: immediate, exponential, linear, constant
    pub retry_strategy: shipper_retry::RetryStrategyType,
    /// Jitter factor for retry delays
    pub retry_jitter: f64,
    /// Per-error-type retry configuration
    pub retry_per_error: shipper_retry::PerErrorConfig,
    pub verify_timeout: Duration,
    pub verify_poll_interval: Duration,
    pub state_dir: PathBuf,
    pub force_resume: bool,
    pub policy: PublishPolicy,
    pub verify_mode: VerifyMode,
    pub readiness: ReadinessConfig,
    pub output_lines: usize,
    /// Force override of existing locks
    pub force: bool,
    /// Lock timeout duration (after which locks are considered stale)
    pub lock_timeout: Duration,
    /// Parallel publishing configuration
    pub parallel: ParallelConfig,
    /// Webhook configuration for publish notifications
    pub webhook: WebhookConfig,
    /// Encryption configuration for state files
    pub encryption: EncryptionSettings,
    /// Target registries for multi-registry publishing
    pub registries: Vec<Registry>,
}

/// A package in the publish plan.
///
/// This represents a single crate that will be published as part of
/// a [`ReleasePlan`]. It contains the minimal information needed to
/// identify and publish the crate.
///
/// # Example
///
/// ```rust
/// use std::path::PathBuf;
/// use shipper::types::PlannedPackage;
///
/// let pkg = PlannedPackage {
///     name: "my-crate".to_string(),
///     version: "1.2.3".to_string(),
///     manifest_path: PathBuf::from("crates/my-crate/Cargo.toml"),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedPackage {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
}

/// A group of packages that can be published in parallel.
///
/// Packages at the same level have no dependencies on each other within
/// the workspace, meaning they can be published concurrently without
/// violating dependency order.
///
/// # Example
///
/// ```rust
/// use std::path::PathBuf;
/// use shipper::types::{PublishLevel, PlannedPackage};
///
/// let level = PublishLevel {
///     level: 0,
///     packages: vec![
///         PlannedPackage {
///             name: "utils".to_string(),
///             version: "1.0.0".to_string(),
///             manifest_path: PathBuf::from("crates/utils/Cargo.toml"),
///         },
///         PlannedPackage {
///             name: "common".to_string(),
///             version: "2.0.0".to_string(),
///             manifest_path: PathBuf::from("crates/common/Cargo.toml"),
///         },
///     ],
/// };
/// ```
///
/// # Level Numbering
///
/// Level 0 contains packages with no workspace dependencies.
/// Level N contains packages that depend only on packages in levels 0..N.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishLevel {
    /// The level number (0 = no dependencies, 1 = depends on level 0, etc.)
    pub level: usize,
    /// Packages that can be published in parallel at this level
    pub packages: Vec<PlannedPackage>,
}

/// A deterministic, identified plan for publishing a workspace.
///
/// The release plan is generated by [`crate::plan::build_plan`] and contains
/// all information needed to execute the publish operation. It includes:
/// - A unique plan ID (SHA256 hash of relevant content)
/// - Ordered list of packages to publish
/// - Dependency information for parallel publishing
/// - Registry configuration
///
/// # Example
///
/// ```ignore
/// let plan = plan::build_plan(&spec)?;
/// println!("Publishing {} packages:", plan.plan.packages.len());
/// for pkg in &plan.plan.packages {
///     println!("  {} {}", pkg.name, pkg.version);
/// }
/// ```
///
/// # Resumability
///
/// The plan ID is stable across runs if the workspace metadata doesn't
/// change. This allows Shipper to detect when a resumed operation is
/// using the same plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePlan {
    pub plan_version: String,
    pub plan_id: String,
    pub created_at: DateTime<Utc>,
    pub registry: Registry,
    /// Packages in publish order (dependencies first).
    pub packages: Vec<PlannedPackage>,
    /// Map of package name -> set of package names it depends on (within the plan).
    /// This is used for level-based parallel publishing.
    #[serde(default)]
    pub dependencies: BTreeMap<String, Vec<String>>,
}

impl ReleasePlan {
    /// Group packages by dependency level for parallel publishing.
    ///
    /// Packages at the same level have no dependencies on each other and can
    /// be published concurrently.
    pub fn group_by_levels(&self) -> Vec<PublishLevel> {
        use std::collections::HashMap;

        if self.packages.is_empty() {
            return Vec::new();
        }

        let mut levels: Vec<PublishLevel> = Vec::new();
        let mut pkg_level: HashMap<String, usize> = HashMap::new();

        for pkg in &self.packages {
            let deps = self
                .dependencies
                .get(&pkg.name)
                .cloned()
                .unwrap_or_default();

            let max_dep_level = deps
                .iter()
                .filter_map(|dep| pkg_level.get(dep).copied())
                .max()
                .unwrap_or(0);

            let level = max_dep_level + 1;
            pkg_level.insert(pkg.name.clone(), level);

            while levels.len() < level {
                levels.push(PublishLevel {
                    level: levels.len(),
                    packages: Vec::new(),
                });
            }

            levels[level - 1].packages.push(pkg.clone());
        }

        levels
    }
}

/// The state of a package in the publish pipeline.
///
/// Each package in a release plan progresses through these states during
/// publishing. The state is persisted to enable resumability after
/// interruption.
///
/// # State Transitions
///
/// ```text
/// Pending → Uploaded → Published
///              ↓
///            Failed
///              ↓
///           Pending (retry)
/// ```
///
/// # Example
///
/// ```rust
/// use shipper::types::PackageState;
///
/// // Initial state
/// let pending = PackageState::Pending;
///
/// // After successful upload
/// let uploaded = PackageState::Uploaded;
///
/// // After visibility verification
/// let published = PackageState::Published;
///
/// // When skipped (e.g., already published)
/// let skipped = PackageState::Skipped {
///     reason: "version already exists".to_string()
/// };
///
/// // On failure
/// let failed = PackageState::Failed {
///     class: shipper::types::ErrorClass::Retryable,
///     message: "network timeout".to_string(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackageState {
    Pending,
    Uploaded,
    Published,
    Skipped { reason: String },
    Failed { class: ErrorClass, message: String },
    Ambiguous { message: String },
}

/// Classification of errors encountered during publishing.
///
/// Error classification determines whether a publish attempt should be
/// retried. Some errors are permanent (retrying won't help) while others
/// are transient (likely to succeed on retry).
///
/// # Example
///
/// ```rust
/// use shipper::types::ErrorClass;
///
/// // Network issues, rate limiting - worth retrying
/// let retryable = ErrorClass::Retryable;
///
/// // Invalid credentials, version conflict - won't succeed on retry
/// let permanent = ErrorClass::Permanent;
///
/// // Unclear - may or may not be retryable
/// let ambiguous = ErrorClass::Ambiguous;
/// ```
///
/// # Classification Heuristics
///
/// Shipper uses various heuristics to classify errors:
/// - HTTP 429 (Too Many Requests) → Retryable
/// - HTTP 401/403 (Auth errors) → Permanent
/// - HTTP 409 (Version conflict) → Permanent
/// - Network timeouts → Retryable
/// - Unknown errors → Ambiguous
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Retryable,
    Permanent,
    Ambiguous,
}

/// Progress tracking for a single package in an execution.
///
/// This struct is persisted to disk during publishing to enable
/// resuming after interruption. It tracks the current state and
/// attempt count for each package.
///
/// # Example
///
/// ```rust
/// use chrono::Utc;
/// use shipper::types::{PackageProgress, PackageState};
///
/// let progress = PackageProgress {
///     name: "my-crate".to_string(),
///     version: "1.2.3".to_string(),
///     attempts: 2,
///     state: PackageState::Pending,
///     last_updated_at: Utc::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageProgress {
    pub name: String,
    pub version: String,
    pub attempts: u32,
    pub state: PackageState,
    pub last_updated_at: DateTime<Utc>,
}

/// The complete state of an in-progress publish operation.
///
/// This is the root structure persisted to disk during publishing.
/// It contains the plan ID, registry info, and progress for all packages.
///
/// # Example
///
/// ```no_run
/// use chrono::Utc;
/// use shipper::types::{ExecutionState, PackageProgress, Registry};
///
/// let state = ExecutionState {
///     state_version: "shipper.state.v1".to_string(),
///     plan_id: "abc123".to_string(),
///     registry: Registry::crates_io(),
///     created_at: Utc::now(),
///     updated_at: Utc::now(),
///     packages: std::collections::BTreeMap::new(),
/// };
///
/// // Save to disk for resumability
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Persistence
///
/// The execution state is saved to `state.json` in the state directory
/// after each package completes. This allows Shipper to resume
/// interrupted operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionState {
    pub state_version: String,
    pub plan_id: String,
    pub registry: Registry,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub packages: BTreeMap<String, PackageProgress>,
}

/// Receipt for a successfully published package.
///
/// This contains all evidence and metadata for a published crate,
/// useful for auditing and debugging. It's part of the final
/// [`Receipt`] document.
///
/// # Example
///
/// ```rust
/// use chrono::Utc;
/// use shipper::types::{PackageReceipt, PackageState, PackageEvidence};
///
/// let receipt = PackageReceipt {
///     name: "my-crate".to_string(),
///     version: "1.2.3".to_string(),
///     attempts: 1,
///     state: PackageState::Published,
///     started_at: Utc::now(),
///     finished_at: Utc::now(),
///     duration_ms: 5000,
///     evidence: PackageEvidence {
///         attempts: vec![],
///         readiness_checks: vec![],
///     },
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageReceipt {
    pub name: String,
    pub version: String,
    pub attempts: u32,
    pub state: PackageState,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: u128,
    pub evidence: PackageEvidence,
}

/// Evidence collected during package publishing.
///
/// This includes detailed information about each publish attempt and
/// readiness verification checks. It's used for debugging and auditing.
///
/// # Contents
///
/// - `attempts`: Details of each publish attempt (command, output, timing)
/// - `readiness_checks`: Results of visibility verification checks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageEvidence {
    pub attempts: Vec<AttemptEvidence>,
    pub readiness_checks: Vec<ReadinessEvidence>,
}

/// Evidence for a single publish attempt.
///
/// Contains the command that was run, its output, and timing information.
/// This is useful for debugging failed publishes.
///
/// # Example
///
/// ```rust
/// use chrono::Utc;
/// use std::time::Duration;
/// use shipper::types::AttemptEvidence;
///
/// let evidence = AttemptEvidence {
///     attempt_number: 1,
///     command: "cargo publish --registry crates-io".to_string(),
///     exit_code: 0,
///     stdout_tail: "Uploading my-crate v1.2.3".to_string(),
///     stderr_tail: "".to_string(),
///     timestamp: Utc::now(),
///     duration: Duration::from_secs(5),
/// };
/// ```
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptEvidence {
    pub attempt_number: u32,
    pub command: String,
    pub exit_code: i32,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub timestamp: DateTime<Utc>,
    #[serde_as(as = "DurationMilliSeconds<u64>")]
    pub duration: Duration,
}

/// Evidence for a single readiness check.
///
/// Records the result of checking crate visibility after publishing.
///
/// # Example
///
/// ```rust
/// use chrono::Utc;
/// use std::time::Duration;
/// use shipper::types::ReadinessEvidence;
///
/// let evidence = ReadinessEvidence {
///     attempt: 1,
///     visible: true,
///     timestamp: Utc::now(),
///     delay_before: Duration::from_secs(2),
/// };
/// ```
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessEvidence {
    pub attempt: u32,
    pub visible: bool,
    pub timestamp: DateTime<Utc>,
    #[serde_as(as = "DurationMilliSeconds<u64>")]
    pub delay_before: Duration,
}

/// Fingerprint of the environment where publishing occurred.
///
/// Captures version information about Shipper, Cargo, Rust, and the
/// operating system. This helps reproduce and debug issues.
///
/// # Example
///
/// ```rust
/// use shipper::types::EnvironmentFingerprint;
///
/// let fp = EnvironmentFingerprint {
///     shipper_version: "0.2.0".to_string(),
///     cargo_version: Some("1.75.0".to_string()),
///     rust_version: Some("1.75.0".to_string()),
///     os: "linux".to_string(),
///     arch: "x86_64".to_string(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentFingerprint {
    pub shipper_version: String,
    pub cargo_version: Option<String>,
    pub rust_version: Option<String>,
    pub os: String,
    pub arch: String,
}

/// Git context at the time of publishing.
///
/// Captures the current git state, including commit hash, branch,
/// tag, and whether the working directory is dirty.
///
/// # Example
///
/// ```rust
/// use shipper::types::GitContext;
///
/// let ctx = GitContext {
///     commit: Some("abc123def".to_string()),
///     branch: Some("main".to_string()),
///     tag: Some("v1.0.0".to_string()),
///     dirty: Some(false),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitContext {
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub dirty: Option<bool>,
}

/// Complete receipt for a publish operation.
///
/// This is the final audit document containing all evidence and
/// metadata for a complete publish operation. It's saved to disk
/// after all packages are published.
///
/// # Example
///
/// ```no_run
/// use chrono::Utc;
/// use std::path::PathBuf;
/// use shipper::types::{Receipt, Registry, EnvironmentFingerprint};
///
/// let receipt = Receipt {
///     receipt_version: "shipper.receipt.v1".to_string(),
///     plan_id: "abc123".to_string(),
///     registry: Registry::crates_io(),
///     started_at: Utc::now(),
///     finished_at: Utc::now(),
///     packages: vec![],
///     event_log_path: PathBuf::from(".shipper/events.jsonl"),
///     git_context: None,
///     environment: EnvironmentFingerprint {
///         shipper_version: env!("CARGO_PKG_VERSION").to_string(),
///         cargo_version: None,
///         rust_version: None,
///         os: std::env::consts::OS.to_string(),
///         arch: std::env::consts::ARCH.to_string(),
///     },
/// };
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Storage
///
/// Receipts are stored in the state directory and can be used for:
/// - Auditing past publishes
/// - Debugging failed publishes
/// - Evidence for compliance requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_version: String,
    pub plan_id: String,
    pub registry: Registry,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub packages: Vec<PackageReceipt>,
    pub event_log_path: PathBuf,
    #[serde(default)]
    pub git_context: Option<GitContext>,
    pub environment: EnvironmentFingerprint,
}

// Event types for evidence-first receipts

/// An event in the publish event log.
///
/// Events are written to an append-only JSONL file during publishing.
/// This provides a detailed timeline for debugging and auditing.
///
/// # Example
///
/// ```rust
/// use chrono::Utc;
/// use shipper::types::{PublishEvent, EventType};
///
/// let event = PublishEvent {
///     timestamp: Utc::now(),
///     event_type: EventType::ExecutionStarted,
///     package: "".to_string(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub package: String, // "name@version"
}

/// Types of events that can occur during publishing.
///
/// These events are logged to provide a complete audit trail of the
/// publish operation. Each variant carries relevant data.
///
/// # Categories
///
/// - **Lifecycle events**: Plan created, execution started/finished
/// - **Package events**: Started, attempted, output, published, failed, skipped
/// - **Readiness events**: Started, polled, completed, timeout
/// - **Preflight events**: Started, verified, ownership checked, completed
///
/// # Example
///
/// ```rust
/// use shipper::types::{EventType, ExecutionResult, ErrorClass, ReadinessMethod, Finishability};
///
/// // Lifecycle events
/// let plan_created = EventType::PlanCreated {
///     plan_id: "abc123".to_string(),
///     package_count: 5,
/// };
/// let started = EventType::ExecutionStarted;
/// let finished = EventType::ExecutionFinished {
///     result: ExecutionResult::Success
/// };
///
/// // Package events
/// let pkg_started = EventType::PackageStarted {
///     name: "my-crate".to_string(),
///     version: "1.0.0".to_string(),
/// };
/// let pkg_failed = EventType::PackageFailed {
///     class: ErrorClass::Retryable,
///     message: "rate limited".to_string(),
/// };
///
/// // Readiness events
/// let ready = EventType::ReadinessStarted {
///     method: ReadinessMethod::Api,
/// };
///
/// // Preflight events
/// let preflight = EventType::PreflightComplete {
///     finishability: Finishability::Proven,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventType {
    // Lifecycle events
    PlanCreated {
        plan_id: String,
        package_count: usize,
    },
    ExecutionStarted,
    ExecutionFinished {
        result: ExecutionResult,
    },

    // Package events
    PackageStarted {
        name: String,
        version: String,
    },
    PackageAttempted {
        attempt: u32,
        command: String,
    },
    PackageOutput {
        stdout_tail: String,
        stderr_tail: String,
    },
    PackagePublished {
        duration_ms: u64,
    },
    PackageFailed {
        class: ErrorClass,
        message: String,
    },
    PackageSkipped {
        reason: String,
    },

    // Readiness events
    ReadinessStarted {
        method: ReadinessMethod,
    },
    ReadinessPoll {
        attempt: u32,
        visible: bool,
    },
    ReadinessComplete {
        duration_ms: u64,
        attempts: u32,
    },
    ReadinessTimeout {
        max_wait_ms: u64,
    },
    // Index readiness events
    IndexReadinessStarted {
        crate_name: String,
        version: String,
    },
    IndexReadinessCheck {
        crate_name: String,
        version: String,
        found: bool,
    },
    IndexReadinessComplete {
        crate_name: String,
        version: String,
        visible: bool,
    },

    // Preflight events
    PreflightStarted,
    PreflightWorkspaceVerify {
        passed: bool,
        output: String,
    },
    PreflightNewCrateDetected {
        crate_name: String,
    },
    PreflightOwnershipCheck {
        crate_name: String,
        verified: bool,
    },
    PreflightComplete {
        finishability: Finishability,
    },
}

/// The result of a publish execution.
///
/// This summarizes the overall outcome of attempting to publish
/// all packages in a release plan.
///
/// # Example
///
/// ```rust
/// use shipper::types::ExecutionResult;
///
/// let success = ExecutionResult::Success;
/// let partial = ExecutionResult::PartialFailure;
/// let complete = ExecutionResult::CompleteFailure;
/// ```
///
/// # Meaning
///
/// - `Success`: All packages published successfully
/// - `PartialFailure`: Some packages failed but others succeeded
/// - `CompleteFailure`: All packages failed (or no packages to publish)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionResult {
    Success,
    PartialFailure,
    CompleteFailure,
}

/// Authentication method used for publishing.
///
/// Shipper supports multiple authentication mechanisms, and this
/// enum tracks which one was used for a particular publish.
///
/// # Example
///
/// ```rust
/// use shipper::types::AuthType;
///
/// let token = AuthType::Token;
/// let trusted = AuthType::TrustedPublishing;
/// let unknown = AuthType::Unknown;
/// ```
///
/// # Authentication Methods
///
/// - `Token`: Traditional Cargo token (CARGO_REGISTRY_TOKEN)
/// - `TrustedPublishing`: GitHub OIDC token from CI/CD
/// - `Unknown`: Could not determine the auth method
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    Token,
    TrustedPublishing,
    Unknown,
}

/// Whether a preflight-verified publish is guaranteed to succeed.
///
/// This is determined during preflight checks based on various
/// factors like whether the crate is new, if ownership is verified, etc.
///
/// # Example
///
/// ```rust
/// use shipper::types::Finishability;
///
/// let proven = Finishability::Proven;       // Should succeed
/// let not_proven = Finishability::NotProven; // Might succeed
/// let failed = Finishability::Failed;        // Won't succeed
/// ```
///
/// # Determination
///
/// - `Proven`: All preflight checks passed strongly (new crate, owned, etc.)
/// - `NotProven`: Some uncertainty (already published version, etc.)
/// - `Failed`: Preflight checks failed (auth issues, dry-run failed, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Finishability {
    Proven,
    NotProven,
    Failed,
}

/// Report from preflight verification checks.
///
/// Before publishing, Shipper runs various preflight checks to catch
/// issues early. This report summarizes the findings.
///
/// # Example
///
/// ```no_run
/// use chrono::Utc;
/// use shipper::types::{PreflightReport, Finishability, PreflightPackage, Registry};
///
/// let report = PreflightReport {
///     plan_id: "abc123".to_string(),
///     token_detected: true,
///     finishability: Finishability::Proven,
///     packages: vec![
///         PreflightPackage {
///             name: "my-crate".to_string(),
///             version: "1.0.0".to_string(),
///             already_published: false,
///             is_new_crate: true,
///             auth_type: Some(shipper::types::AuthType::Token),
///             ownership_verified: true,
///             dry_run_passed: true,
///         },
///     ],
///     timestamp: Utc::now(),
/// };
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Usage
///
/// The preflight report is used to:
/// - Determine if publishing should proceed
/// - Provide transparency about potential issues
/// - Support debugging if publish fails
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub plan_id: String,
    pub token_detected: bool,
    pub finishability: Finishability,
    pub packages: Vec<PreflightPackage>,
    pub timestamp: DateTime<Utc>,
}

/// Preflight status for a single package.
///
/// Contains the results of preflight checks for one crate in the
/// workspace.
///
/// # Example
///
/// ```rust
/// use shipper::types::{PreflightPackage, AuthType};
///
/// let pkg = PreflightPackage {
///     name: "my-crate".to_string(),
///     version: "1.0.0".to_string(),
///     already_published: false,
///     is_new_crate: true,
///     auth_type: Some(AuthType::Token),
///     ownership_verified: true,
///     dry_run_passed: true,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightPackage {
    pub name: String,
    pub version: String,
    pub already_published: bool,
    pub is_new_crate: bool,
    pub auth_type: Option<AuthType>,
    pub ownership_verified: bool,
    pub dry_run_passed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crates_io_registry_defaults_are_expected() {
        let reg = Registry::crates_io();
        assert_eq!(reg.name, "crates-io");
        assert_eq!(reg.api_base, "https://crates.io");
    }

    #[test]
    fn uploaded_state_serde_roundtrip() {
        let st = PackageState::Uploaded;
        let json = serde_json::to_string(&st).expect("serialize");
        assert_eq!(json, r#"{"state":"uploaded"}"#);
        let rt: PackageState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(rt, PackageState::Uploaded);
    }

    #[test]
    fn package_state_serializes_with_tagged_representation() {
        let st = PackageState::Failed {
            class: ErrorClass::Permanent,
            message: "nope".to_string(),
        };

        let json = serde_json::to_string(&st).expect("serialize");
        assert!(json.contains("\"state\":\"failed\""));
        assert!(json.contains("\"class\":\"permanent\""));

        let rt: PackageState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(rt, st);
    }

    #[test]
    fn execution_state_roundtrips_json() {
        let mut packages = BTreeMap::new();
        packages.insert(
            "demo@1.2.3".to_string(),
            PackageProgress {
                name: "demo".to_string(),
                version: "1.2.3".to_string(),
                attempts: 2,
                state: PackageState::Published,
                last_updated_at: Utc::now(),
            },
        );

        let st = ExecutionState {
            state_version: "shipper.state.v1".to_string(),
            plan_id: "plan-1".to_string(),
            registry: Registry::crates_io(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            packages,
        };

        let json = serde_json::to_string_pretty(&st).expect("serialize");
        let parsed: ExecutionState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.plan_id, "plan-1");
        assert!(parsed.packages.contains_key("demo@1.2.3"));
    }

    #[test]
    fn registry_get_index_base_strips_sparse_prefix() {
        let registry = Registry {
            name: "crates-io".to_string(),
            api_base: "https://crates.io".to_string(),
            index_base: Some("sparse+https://index.crates.io".to_string()),
        };

        assert_eq!(registry.get_index_base(), "https://index.crates.io");
    }

    #[test]
    fn readiness_method_default_is_api() {
        let method = ReadinessMethod::default();
        assert_eq!(method, ReadinessMethod::Api);
    }

    #[test]
    fn readiness_config_default_values() {
        let config = ReadinessConfig::default();
        assert!(config.enabled);
        assert_eq!(config.method, ReadinessMethod::Api);
        assert_eq!(config.initial_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(60));
        assert_eq!(config.max_total_wait, Duration::from_secs(300));
        assert_eq!(config.poll_interval, Duration::from_secs(2));
        assert_eq!(config.jitter_factor, 0.5);
    }

    #[test]
    fn readiness_config_can_be_customized() {
        let config = ReadinessConfig {
            enabled: false,
            method: ReadinessMethod::Both,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            max_total_wait: Duration::from_secs(600),
            poll_interval: Duration::from_secs(5),
            jitter_factor: 0.25,
            index_path: None,
            prefer_index: false,
        };
        assert!(!config.enabled);
        assert_eq!(config.method, ReadinessMethod::Both);
        assert_eq!(config.initial_delay, Duration::from_millis(500));
        assert_eq!(config.max_delay, Duration::from_secs(30));
        assert_eq!(config.max_total_wait, Duration::from_secs(600));
        assert_eq!(config.poll_interval, Duration::from_secs(5));
        assert_eq!(config.jitter_factor, 0.25);
    }

    // Property-based tests using proptest

    #[cfg(test)]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            // Preflight report serialization/deserialization roundtrip
            #[test]
            fn preflight_report_roundtrip(
                plan_id in "[a-z0-9-]+",
                token_detected in any::<bool>(),
                finishability_variant in 0u8..3,
                package_count in 0usize..10,
            ) {
                let finishability = match finishability_variant {
                    0 => Finishability::Proven,
                    1 => Finishability::NotProven,
                    _ => Finishability::Failed,
                };

                let packages: Vec<PreflightPackage> = (0..package_count)
                    .map(|i| PreflightPackage {
                        name: format!("crate-{}", i),
                        version: format!("0.{}.0", i),
                        already_published: i % 2 == 0,
                        is_new_crate: i % 3 == 0,
                        auth_type: if i % 2 == 0 { Some(AuthType::Token) } else { None },
                        ownership_verified: i % 3 != 0,
                        dry_run_passed: i % 5 != 0,
                    })
                    .collect();

                let report = PreflightReport {
                    plan_id: plan_id.clone(),
                    token_detected,
                    finishability,
                    packages: packages.clone(),
                    timestamp: Utc::now(),
                };

                // Serialize and deserialize
                let json = serde_json::to_string(&report).unwrap();
                let parsed: PreflightReport = serde_json::from_str(&json).unwrap();

                // Verify roundtrip
                assert_eq!(parsed.plan_id, report.plan_id);
                assert_eq!(parsed.token_detected, report.token_detected);
                assert_eq!(parsed.finishability, report.finishability);
                assert_eq!(parsed.packages.len(), report.packages.len());
                for (orig, parsed_pkg) in report.packages.iter().zip(parsed.packages.iter()) {
                    assert_eq!(parsed_pkg.name, orig.name);
                    assert_eq!(parsed_pkg.version, orig.version);
                    assert_eq!(parsed_pkg.already_published, orig.already_published);
                    assert_eq!(parsed_pkg.is_new_crate, orig.is_new_crate);
                    assert_eq!(parsed_pkg.auth_type, orig.auth_type);
                    assert_eq!(parsed_pkg.ownership_verified, orig.ownership_verified);
                    assert_eq!(parsed_pkg.dry_run_passed, orig.dry_run_passed);
                }
            }

            // Preflight package serialization roundtrip
            #[test]
            fn preflight_package_roundtrip(
                name in "[a-z][a-z0-9-]*",
                version in "[0-9]+\\.[0-9]+\\.[0-9]+",
                already_published in any::<bool>(),
                is_new_crate in any::<bool>(),
                auth_type_variant in 0u8..4,
                ownership_verified in any::<bool>(),
                dry_run_passed in any::<bool>(),
            ) {
                let auth_type = match auth_type_variant {
                    0 => Some(AuthType::Token),
                    1 => Some(AuthType::TrustedPublishing),
                    2 => Some(AuthType::Unknown),
                    _ => None,
                };

                let pkg = PreflightPackage {
                    name: name.clone(),
                    version: version.clone(),
                    already_published,
                    is_new_crate,
                    auth_type: auth_type.clone(),
                    ownership_verified,
                    dry_run_passed,
                };

                // Serialize and deserialize
                let json = serde_json::to_string(&pkg).unwrap();
                let parsed: PreflightPackage = serde_json::from_str(&json).unwrap();

                // Verify roundtrip
                assert_eq!(parsed.name, pkg.name);
                assert_eq!(parsed.version, pkg.version);
                assert_eq!(parsed.already_published, pkg.already_published);
                assert_eq!(parsed.is_new_crate, pkg.is_new_crate);
                assert_eq!(parsed.auth_type, pkg.auth_type);
                assert_eq!(parsed.ownership_verified, pkg.ownership_verified);
                assert_eq!(parsed.dry_run_passed, pkg.dry_run_passed);
            }

            // AuthType serialization roundtrip
            #[test]
            fn auth_type_roundtrip(auth_type_variant in 0u8..3) {
                let auth_type = match auth_type_variant {
                    0 => AuthType::Token,
                    1 => AuthType::TrustedPublishing,
                    _ => AuthType::Unknown,
                };

                let json = serde_json::to_string(&auth_type).unwrap();
                let parsed: AuthType = serde_json::from_str(&json).unwrap();

                assert_eq!(parsed, auth_type);
            }

            // Finishability serialization roundtrip
            #[test]
            fn finishability_roundtrip(finishability_variant in 0u8..3) {
                let finishability = match finishability_variant {
                    0 => Finishability::Proven,
                    1 => Finishability::NotProven,
                    _ => Finishability::Failed,
                };

                let json = serde_json::to_string(&finishability).unwrap();
                let parsed: Finishability = serde_json::from_str(&json).unwrap();

                assert_eq!(parsed, finishability);
            }

            // EnvironmentFingerprint serialization roundtrip
            #[test]
            fn environment_fingerprint_roundtrip(
                shipper_version in "[0-9]+\\.[0-9]+\\.[0-9]+",
                cargo_version in prop::option::of("[0-9]+\\.[0-9]+\\.[0-9]+"),
                rust_version in prop::option::of("[0-9]+\\.[0-9]+\\.[0-9]+"),
                os in "[a-z]+",
                arch in "[a-z0-9_]+",
            ) {
                let fingerprint = EnvironmentFingerprint {
                    shipper_version: shipper_version.clone(),
                    cargo_version: cargo_version.clone(),
                    rust_version: rust_version.clone(),
                    os: os.clone(),
                    arch: arch.clone(),
                };

                // Serialize and deserialize
                let json = serde_json::to_string(&fingerprint).unwrap();
                let parsed: EnvironmentFingerprint = serde_json::from_str(&json).unwrap();

                // Verify roundtrip
                assert_eq!(parsed.shipper_version, fingerprint.shipper_version);
                assert_eq!(parsed.cargo_version, fingerprint.cargo_version);
                assert_eq!(parsed.rust_version, fingerprint.rust_version);
                assert_eq!(parsed.os, fingerprint.os);
                assert_eq!(parsed.arch, fingerprint.arch);
            }

            // GitContext serialization roundtrip
            #[test]
            fn git_context_roundtrip(
                commit in prop::option::of("[a-f0-9]+"),
                branch in prop::option::of("[a-z0-9-]+"),
                tag in prop::option::of("[a-z0-9-\\.]+"),
                dirty in prop::option::of(any::<bool>()),
            ) {
                let git_context = GitContext {
                    commit: commit.clone(),
                    branch: branch.clone(),
                    tag: tag.clone(),
                    dirty,
                };

                // Serialize and deserialize
                let json = serde_json::to_string(&git_context).unwrap();
                let parsed: GitContext = serde_json::from_str(&json).unwrap();

                // Verify roundtrip
                assert_eq!(parsed.commit, git_context.commit);
                assert_eq!(parsed.branch, git_context.branch);
                assert_eq!(parsed.tag, git_context.tag);
                assert_eq!(parsed.dirty, git_context.dirty);
            }

            // Registry serialization roundtrip
            #[test]
            fn registry_roundtrip(
                name in "[a-z0-9-]+",
                api_base in "https?://[a-z0-9.-]+",
                index_base in prop::option::of("https?://[a-z0-9.-]+"),
            ) {
                let registry = Registry {
                    name: name.clone(),
                    api_base: api_base.clone(),
                    index_base: index_base.clone(),
                };

                // Serialize and deserialize
                let json = serde_json::to_string(&registry).unwrap();
                let parsed: Registry = serde_json::from_str(&json).unwrap();

                // Verify roundtrip
                assert_eq!(parsed.name, registry.name);
                assert_eq!(parsed.api_base, registry.api_base);
                assert_eq!(parsed.index_base, registry.index_base);
            }

            // ReadinessConfig serialization roundtrip
            #[test]
            fn readiness_config_roundtrip(
                enabled in any::<bool>(),
                method_variant in 0u8..3,
                initial_delay_ms in 0u64..10000,
                max_delay_ms in 0u64..100000,
                max_total_wait_ms in 0u64..1000000,
                poll_interval_ms in 0u64..10000,
                jitter_factor in 0.0f64..1.0,
                prefer_index in any::<bool>(),
            ) {
                let method = match method_variant {
                    0 => ReadinessMethod::Api,
                    1 => ReadinessMethod::Index,
                    _ => ReadinessMethod::Both,
                };

                let config = ReadinessConfig {
                    enabled,
                    method,
                    initial_delay: Duration::from_millis(initial_delay_ms),
                    max_delay: Duration::from_millis(max_delay_ms),
                    max_total_wait: Duration::from_millis(max_total_wait_ms),
                    poll_interval: Duration::from_millis(poll_interval_ms),
                    jitter_factor,
                    index_path: None,
                    prefer_index,
                };

                // Serialize and deserialize
                let json = serde_json::to_string(&config).unwrap();
                let parsed: ReadinessConfig = serde_json::from_str(&json).unwrap();

                // Verify roundtrip
                assert_eq!(parsed.enabled, config.enabled);
                assert_eq!(parsed.method, config.method);
                assert_eq!(parsed.initial_delay, config.initial_delay);
                assert_eq!(parsed.max_delay, config.max_delay);
                assert_eq!(parsed.max_total_wait, config.max_total_wait);
                assert_eq!(parsed.poll_interval, config.poll_interval);
                assert!((parsed.jitter_factor - config.jitter_factor).abs() < 1e-10,
                    "jitter_factor mismatch: {} vs {}", parsed.jitter_factor, config.jitter_factor);
                assert_eq!(parsed.prefer_index, config.prefer_index);
            }

            // Index path calculation is deterministic
            #[test]
            fn index_path_deterministic(crate_name in "[a-z0-9-]+") {
                // Calculate the index path twice and verify it's the same
                let first = calculate_index_path_for_crate(&crate_name);
                let second = calculate_index_path_for_crate(&crate_name);
                assert_eq!(first, second, "Index path calculation should be deterministic");
            }

            // Index path follows Cargo's sparse index scheme
            #[test]
            fn index_path_follows_pattern(crate_name in "[a-z0-9-]{3,20}") {
                let path = calculate_index_path_for_crate(&crate_name);
                let lower = crate_name.to_lowercase();
                let parts: Vec<&str> = path.split('/').collect();

                match lower.len() {
                    3 => {
                        assert_eq!(parts.len(), 3, "3-char crate should have 3 parts");
                        assert_eq!(parts[0], "3");
                        assert_eq!(parts[1], &lower[..1]);
                        assert_eq!(parts[2], lower);
                    }
                    n if n >= 4 => {
                        assert_eq!(parts.len(), 3, "4+ char crate should have 3 parts");
                        assert_eq!(parts[0], &lower[..2]);
                        assert_eq!(parts[1], &lower[2..4]);
                        assert_eq!(parts[2], lower);
                    }
                    _ => unreachable!("regex guarantees at least 3 chars"),
                }
            }

            // Schema version parsing is deterministic
            #[test]
            fn schema_version_parsing_deterministic(
                middle in "[a-z]+",
                version_num in 1u32..1000,
            ) {
                let version_str = format!("shipper.{}.v{}", middle, version_num);

                let first = parse_schema_version_for_test(&version_str);
                let second = parse_schema_version_for_test(&version_str);

                assert_eq!(first, second, "Schema version parsing should be deterministic");
                assert_eq!(first, Ok(version_num));
            }
        }

        // Helper functions for property-based tests

        fn calculate_index_path_for_crate(crate_name: &str) -> String {
            let lower = crate_name.to_lowercase();
            match lower.len() {
                1 => format!("1/{}", lower),
                2 => format!("2/{}", lower),
                3 => format!("3/{}/{}", &lower[..1], lower),
                _ => format!("{}/{}/{}", &lower[..2], &lower[2..4], lower),
            }
        }

        fn parse_schema_version_for_test(version: &str) -> Result<u32, String> {
            let parts: Vec<&str> = version.split('.').collect();
            if parts.len() != 3 || !parts[0].starts_with("shipper") || !parts[2].starts_with('v') {
                return Err("invalid format".to_string());
            }

            let version_part = &parts[2][1..];
            version_part.parse::<u32>().map_err(|e| e.to_string())
        }
    }
}
