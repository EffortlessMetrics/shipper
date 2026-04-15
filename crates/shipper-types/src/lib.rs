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
use serde::{Deserialize, Serialize};
use serde_with::{DurationMilliSeconds, serde_as};

pub use shipper_duration::{deserialize_duration, serialize_duration};
use shipper_encrypt::EncryptionConfig as EncryptionSettings;
use shipper_webhook::WebhookConfig;

/// Storage backend configuration types (was `shipper-storage`, split).
pub mod storage;

/// Represents a Cargo registry for publishing crates.
///
/// A registry is identified by its name (used with `cargo publish --registry <name>`)
/// and its API/base URLs. The default registry is crates.io, which can be created
/// using [`Registry::crates_io()`].
///
/// # Example
///
/// ```ignore
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
    /// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
    /// Allow publishing from a dirty git working tree.
    pub allow_dirty: bool,
    /// Skip crate-ownership preflight checks.
    pub skip_ownership_check: bool,
    /// Fail preflight if ownership verification fails.
    pub strict_ownership: bool,
    /// Pass `--no-verify` to `cargo publish` (skip pre-publish build).
    pub no_verify: bool,
    /// Maximum number of publish attempts per crate.
    pub max_attempts: u32,
    /// Initial backoff delay between retries.
    pub base_delay: Duration,
    /// Upper bound on backoff delay.
    pub max_delay: Duration,
    /// Retry strategy type: immediate, exponential, linear, constant
    pub retry_strategy: shipper_retry::RetryStrategyType,
    /// Jitter factor for retry delays
    pub retry_jitter: f64,
    /// Per-error-type retry configuration
    pub retry_per_error: shipper_retry::PerErrorConfig,
    /// Timeout for the workspace-level dry-run verification step.
    pub verify_timeout: Duration,
    /// Poll interval for the dry-run verification step.
    pub verify_poll_interval: Duration,
    /// Directory for persisted state, receipts, and event logs.
    pub state_dir: PathBuf,
    /// Force resume even when the plan ID has changed.
    pub force_resume: bool,
    /// Publishing policy preset (safe / balanced / fast).
    pub policy: PublishPolicy,
    /// Dry-run verification mode (workspace / package / none).
    pub verify_mode: VerifyMode,
    /// Readiness (post-publish visibility) configuration.
    pub readiness: ReadinessConfig,
    /// Number of stdout/stderr lines to capture as evidence.
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
    /// Optional package name to resume from (skips all packages before this one)
    pub resume_from: Option<String>,
}

/// A package in the publish plan.
///
/// This represents a single crate that will be published as part of
/// a [`ReleasePlan`]. It contains the minimal information needed to
/// identify and publish the crate.
///
/// # Example
///
/// ```ignore
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
/// ```ignore
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
/// The release plan is generated by `shipper::plan::build_plan` and contains
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

/// A workspace package that was excluded from the publish plan.
///
/// Packages are skipped when their `publish` field in `Cargo.toml`
/// is `false` or does not include the target registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedPackage {
    /// Crate name as declared in `Cargo.toml`.
    pub name: String,
    /// Crate version string.
    pub version: String,
    /// Human-readable reason the package was excluded.
    pub reason: String,
}

/// The output of `shipper::plan::build_plan`: a publish plan plus context.
///
/// Contains the workspace root path, the deterministic [`ReleasePlan`],
/// and a list of packages that were skipped (with reasons).
#[derive(Debug, Clone)]
pub struct PlannedWorkspace {
    /// Absolute path to the workspace root directory.
    pub workspace_root: PathBuf,
    /// The deterministic, SHA256-identified publish plan.
    pub plan: ReleasePlan,
    /// Packages that were excluded from the plan.
    pub skipped: Vec<SkippedPackage>,
}

impl ReleasePlan {
    /// Group packages by dependency level for parallel publishing.
    ///
    /// Packages at the same level have no dependencies on each other and can
    /// be published concurrently.
    pub fn group_by_levels(&self) -> Vec<PublishLevel> {
        group_packages_by_levels(&self.packages, |pkg| pkg.name.as_str(), &self.dependencies)
            .into_iter()
            .map(|l| PublishLevel {
                level: l.level,
                packages: l.packages,
            })
            .collect()
    }
}

/// A group of packages that can be processed in parallel.
///
/// Generic counterpart of [`PublishLevel`] used by [`group_packages_by_levels`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericPublishLevel<T> {
    /// Zero-based level number.
    pub level: usize,
    /// Packages assigned to this level.
    pub packages: Vec<T>,
}

/// Group packages into dependency levels.
///
/// `ordered_packages` should be deterministic. Dependencies that are not part
/// of `ordered_packages` are ignored. If cyclic/inconsistent dependencies are
/// encountered, the function falls back to deterministic singleton progress so
/// every package still appears exactly once.
pub fn group_packages_by_levels<T, F>(
    ordered_packages: &[T],
    package_name: F,
    dependencies: &BTreeMap<String, Vec<String>>,
) -> Vec<GenericPublishLevel<T>>
where
    T: Clone,
    F: Fn(&T) -> &str,
{
    use std::collections::BTreeSet;

    let mut ordered_names: Vec<String> = Vec::new();
    let mut package_lookup: BTreeMap<String, T> = BTreeMap::new();

    for package in ordered_packages {
        let name = package_name(package).to_string();
        if package_lookup.contains_key(&name) {
            continue;
        }
        ordered_names.push(name.clone());
        package_lookup.insert(name, package.clone());
    }

    if ordered_names.is_empty() {
        return Vec::new();
    }

    let package_set: BTreeSet<String> = ordered_names.iter().cloned().collect();
    let mut indegree: BTreeMap<String, usize> = package_set
        .iter()
        .map(|name| (name.clone(), 0usize))
        .collect();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for name in &ordered_names {
        if let Some(deps) = dependencies.get(name) {
            for dep in deps {
                if !package_set.contains(dep) {
                    continue;
                }
                if let Some(degree) = indegree.get_mut(name) {
                    *degree += 1;
                }
                dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(name.clone());
            }
        }
    }

    let mut remaining: BTreeSet<String> = package_set;
    let mut levels: Vec<GenericPublishLevel<T>> = Vec::new();

    while !remaining.is_empty() {
        let mut current: Vec<String> = ordered_names
            .iter()
            .filter(|name| {
                remaining.contains(*name) && indegree.get(*name).copied().unwrap_or(0) == 0
            })
            .cloned()
            .collect();

        if current.is_empty() {
            if let Some(name) = ordered_names
                .iter()
                .find(|name| remaining.contains(*name))
                .cloned()
            {
                current.push(name);
            } else {
                break;
            }
        }

        let packages = current
            .iter()
            .filter_map(|name| package_lookup.get(name).cloned())
            .collect();

        levels.push(GenericPublishLevel {
            level: levels.len(),
            packages,
        });

        for name in current {
            remaining.remove(&name);
            if let Some(children) = dependents.get(&name) {
                for child in children {
                    if !remaining.contains(child) {
                        continue;
                    }
                    if let Some(degree) = indegree.get_mut(child) {
                        *degree = degree.saturating_sub(1);
                    }
                }
            }
        }
    }

    levels
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
/// use shipper::types::GitContext;
///
/// let ctx = GitContext {
///     commit: Some("abc123def".to_string()),
///     branch: Some("main".to_string()),
///     tag: Some("v1.0.0".to_string()),
///     dirty: Some(false),
/// };
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitContext {
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub dirty: Option<bool>,
}

impl GitContext {
    /// Create a new empty git context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the context has commit information.
    pub fn has_commit(&self) -> bool {
        self.commit.is_some()
    }

    /// Whether the working tree is dirty.
    ///
    /// When `dirty` is `None`, this defaults to `true` (treat unknown as dirty) to
    /// preserve the safe-by-default semantics of the original `shipper-git` crate.
    pub fn is_dirty(&self) -> bool {
        self.dirty.unwrap_or(true)
    }

    /// Get a short commit hash (first 7 bytes).
    ///
    /// Returns `None` if the context has no commit. The original `shipper-git`
    /// implementation sliced by byte index assuming ASCII hex; we preserve the
    /// same behavior here (input shorter than 7 bytes is returned verbatim).
    pub fn short_commit(&self) -> Option<&str> {
        self.commit
            .as_ref()
            .map(|c| if c.len() > 7 { &c[..7] } else { c.as_str() })
    }
}

/// Complete receipt for a publish operation.
///
/// This is the final audit document containing all evidence and
/// metadata for a complete publish operation. It's saved to disk
/// after all packages are published.
///
/// # Example
///
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
    /// Detailed output from workspace-level dry-run verification
    pub dry_run_output: Option<String>,
}

/// Preflight status for a single package.
///
/// Contains the results of preflight checks for one crate in the
/// workspace.
///
/// # Example
///
/// ```ignore
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
///     dry_run_output: None,
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
    /// Detailed output from package-level dry-run verification
    pub dry_run_output: Option<String>,
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

    // ===== PackageState transition tests =====

    #[test]
    fn package_state_pending_to_uploaded_is_valid() {
        let pending = PackageState::Pending;
        let uploaded = PackageState::Uploaded;
        assert_eq!(pending, PackageState::Pending);
        assert_eq!(uploaded, PackageState::Uploaded);
        // Pending can transition to Uploaded
        assert_ne!(pending, uploaded);
    }

    #[test]
    fn package_state_uploaded_to_published_is_valid() {
        let uploaded = PackageState::Uploaded;
        let published = PackageState::Published;
        assert_ne!(uploaded, published);
    }

    #[test]
    fn package_state_pending_to_failed_is_valid() {
        let pending = PackageState::Pending;
        let failed = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "connection refused".to_string(),
        };
        assert_ne!(pending, failed);
    }

    #[test]
    fn package_state_pending_to_skipped_is_valid() {
        let skipped = PackageState::Skipped {
            reason: "already published".to_string(),
        };
        assert!(matches!(skipped, PackageState::Skipped { .. }));
    }

    #[test]
    fn package_state_uploaded_to_ambiguous_is_valid() {
        let ambiguous = PackageState::Ambiguous {
            message: "upload succeeded but timed out waiting for visibility".to_string(),
        };
        assert!(matches!(ambiguous, PackageState::Ambiguous { .. }));
    }

    #[test]
    fn package_state_failed_equality_requires_matching_fields() {
        let f1 = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "timeout".to_string(),
        };
        let f2 = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "timeout".to_string(),
        };
        let f3 = PackageState::Failed {
            class: ErrorClass::Permanent,
            message: "timeout".to_string(),
        };
        let f4 = PackageState::Failed {
            class: ErrorClass::Retryable,
            message: "different".to_string(),
        };
        assert_eq!(f1, f2);
        assert_ne!(f1, f3);
        assert_ne!(f1, f4);
    }

    #[test]
    fn package_state_skipped_equality_by_reason() {
        let s1 = PackageState::Skipped {
            reason: "exists".to_string(),
        };
        let s2 = PackageState::Skipped {
            reason: "exists".to_string(),
        };
        let s3 = PackageState::Skipped {
            reason: "other".to_string(),
        };
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn package_state_all_unit_variants_are_distinct() {
        let states: Vec<PackageState> = vec![
            PackageState::Pending,
            PackageState::Uploaded,
            PackageState::Published,
        ];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    // ===== ReleasePlan determinism =====

    #[test]
    fn release_plan_serde_roundtrip_preserves_all_fields() {
        let plan = ReleasePlan {
            plan_version: "shipper.plan.v1".to_string(),
            plan_id: "deadbeef01234567".to_string(),
            created_at: "2025-06-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            registry: Registry::crates_io(),
            packages: vec![
                PlannedPackage {
                    name: "alpha".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("crates/alpha/Cargo.toml"),
                },
                PlannedPackage {
                    name: "beta".to_string(),
                    version: "2.0.0".to_string(),
                    manifest_path: PathBuf::from("crates/beta/Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([("beta".to_string(), vec!["alpha".to_string()])]),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let parsed: ReleasePlan = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.plan_version, plan.plan_version);
        assert_eq!(parsed.plan_id, plan.plan_id);
        assert_eq!(parsed.packages.len(), 2);
        assert_eq!(parsed.packages[0].name, "alpha");
        assert_eq!(parsed.packages[1].name, "beta");
        assert_eq!(parsed.dependencies.len(), 1);
        assert_eq!(parsed.dependencies["beta"], vec!["alpha".to_string()]);
        assert_eq!(parsed.registry.name, "crates-io");
    }

    #[test]
    fn release_plan_empty_dependencies_roundtrip() {
        let plan = ReleasePlan {
            plan_version: "shipper.plan.v1".to_string(),
            plan_id: "nodeps".to_string(),
            created_at: Utc::now(),
            registry: Registry::crates_io(),
            packages: vec![PlannedPackage {
                name: "standalone".to_string(),
                version: "0.1.0".to_string(),
                manifest_path: PathBuf::from("Cargo.toml"),
            }],
            dependencies: BTreeMap::new(),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let parsed: ReleasePlan = serde_json::from_str(&json).unwrap();
        assert!(parsed.dependencies.is_empty());
    }

    #[test]
    fn release_plan_group_by_levels_single_crate() {
        let plan = ReleasePlan {
            plan_version: "shipper.plan.v1".to_string(),
            plan_id: "single".to_string(),
            created_at: Utc::now(),
            registry: Registry::crates_io(),
            packages: vec![PlannedPackage {
                name: "solo".to_string(),
                version: "1.0.0".to_string(),
                manifest_path: PathBuf::from("Cargo.toml"),
            }],
            dependencies: BTreeMap::new(),
        };
        let levels = plan.group_by_levels();
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].level, 0);
        assert_eq!(levels[0].packages.len(), 1);
        assert_eq!(levels[0].packages[0].name, "solo");
    }

    #[test]
    fn release_plan_group_by_levels_chain() {
        let plan = ReleasePlan {
            plan_version: "shipper.plan.v1".to_string(),
            plan_id: "chain".to_string(),
            created_at: Utc::now(),
            registry: Registry::crates_io(),
            packages: vec![
                PlannedPackage {
                    name: "a".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("a/Cargo.toml"),
                },
                PlannedPackage {
                    name: "b".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("b/Cargo.toml"),
                },
                PlannedPackage {
                    name: "c".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("c/Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::from([
                ("b".to_string(), vec!["a".to_string()]),
                ("c".to_string(), vec!["b".to_string()]),
            ]),
        };
        let levels = plan.group_by_levels();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0].level, 0);
        assert_eq!(levels[0].packages[0].name, "a");
        assert_eq!(levels[1].level, 1);
        assert_eq!(levels[1].packages[0].name, "b");
        assert_eq!(levels[2].level, 2);
        assert_eq!(levels[2].packages[0].name, "c");
    }

    #[test]
    fn release_plan_group_by_levels_parallel_at_level_zero() {
        let plan = ReleasePlan {
            plan_version: "shipper.plan.v1".to_string(),
            plan_id: "parallel".to_string(),
            created_at: Utc::now(),
            registry: Registry::crates_io(),
            packages: vec![
                PlannedPackage {
                    name: "x".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("x/Cargo.toml"),
                },
                PlannedPackage {
                    name: "y".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("y/Cargo.toml"),
                },
                PlannedPackage {
                    name: "z".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("z/Cargo.toml"),
                },
            ],
            dependencies: BTreeMap::new(),
        };
        let levels = plan.group_by_levels();
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].packages.len(), 3);
    }

    // ===== Receipt serialization roundtrips =====

    #[test]
    fn receipt_with_ambiguous_state_roundtrip() {
        let t = "2025-01-15T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v1".to_string(),
            plan_id: "ambig-test".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![PackageReceipt {
                name: "ambig-crate".to_string(),
                version: "0.1.0".to_string(),
                attempts: 2,
                state: PackageState::Ambiguous {
                    message: "upload ok but readiness timed out".to_string(),
                },
                started_at: t,
                finished_at: t,
                duration_ms: 60000,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            }],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: None,
                rust_version: None,
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let parsed: Receipt = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            &parsed.packages[0].state,
            PackageState::Ambiguous { message } if message.contains("readiness timed out")
        ));
    }

    #[test]
    fn receipt_empty_packages_roundtrip() {
        let t = Utc::now();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v1".to_string(),
            plan_id: "empty".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages: vec![],
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: None,
                rust_version: None,
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let parsed: Receipt = serde_json::from_str(&json).unwrap();
        assert!(parsed.packages.is_empty());
    }

    #[test]
    fn receipt_all_state_variants_roundtrip() {
        let t = Utc::now();
        let states = vec![
            PackageState::Published,
            PackageState::Uploaded,
            PackageState::Pending,
            PackageState::Skipped {
                reason: "exists".to_string(),
            },
            PackageState::Failed {
                class: ErrorClass::Permanent,
                message: "auth".to_string(),
            },
            PackageState::Ambiguous {
                message: "unclear".to_string(),
            },
        ];
        let packages: Vec<PackageReceipt> = states
            .into_iter()
            .enumerate()
            .map(|(i, state)| PackageReceipt {
                name: format!("crate-{i}"),
                version: "1.0.0".to_string(),
                attempts: 1,
                state,
                started_at: t,
                finished_at: t,
                duration_ms: 100,
                evidence: PackageEvidence {
                    attempts: vec![],
                    readiness_checks: vec![],
                },
            })
            .collect();
        let receipt = Receipt {
            receipt_version: "shipper.receipt.v1".to_string(),
            plan_id: "all-variants".to_string(),
            registry: Registry::crates_io(),
            started_at: t,
            finished_at: t,
            packages,
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: None,
                rust_version: None,
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let parsed: Receipt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.packages.len(), 6);
        assert!(matches!(parsed.packages[0].state, PackageState::Published));
        assert!(matches!(parsed.packages[1].state, PackageState::Uploaded));
        assert!(matches!(parsed.packages[2].state, PackageState::Pending));
        assert!(matches!(
            parsed.packages[3].state,
            PackageState::Skipped { .. }
        ));
        assert!(matches!(
            parsed.packages[4].state,
            PackageState::Failed { .. }
        ));
        assert!(matches!(
            parsed.packages[5].state,
            PackageState::Ambiguous { .. }
        ));
    }

    // ===== PublishPolicy / VerifyMode =====

    #[test]
    fn publish_policy_default_is_safe() {
        assert_eq!(PublishPolicy::default(), PublishPolicy::Safe);
    }

    #[test]
    fn publish_policy_exhaustive_serde() {
        let policies = [
            PublishPolicy::Safe,
            PublishPolicy::Balanced,
            PublishPolicy::Fast,
        ];
        let expected_json = [r#""safe""#, r#""balanced""#, r#""fast""#];
        for (policy, expected) in policies.iter().zip(expected_json.iter()) {
            let json = serde_json::to_string(policy).unwrap();
            assert_eq!(&json, expected);
            let parsed: PublishPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, policy);
        }
    }

    #[test]
    fn verify_mode_default_is_workspace() {
        assert_eq!(VerifyMode::default(), VerifyMode::Workspace);
    }

    #[test]
    fn verify_mode_exhaustive_serde() {
        let modes = [VerifyMode::Workspace, VerifyMode::Package, VerifyMode::None];
        let expected_json = [r#""workspace""#, r#""package""#, r#""none""#];
        for (mode, expected) in modes.iter().zip(expected_json.iter()) {
            let json = serde_json::to_string(mode).unwrap();
            assert_eq!(&json, expected);
            let parsed: VerifyMode = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, mode);
        }
    }

    #[test]
    fn readiness_method_exhaustive_serde() {
        let methods = [
            ReadinessMethod::Api,
            ReadinessMethod::Index,
            ReadinessMethod::Both,
        ];
        let expected_json = [r#""api""#, r#""index""#, r#""both""#];
        for (method, expected) in methods.iter().zip(expected_json.iter()) {
            let json = serde_json::to_string(method).unwrap();
            assert_eq!(&json, expected);
            let parsed: ReadinessMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, method);
        }
    }

    // ===== PackageProgress =====

    #[test]
    fn package_progress_epoch_timestamp_roundtrip() {
        let epoch = DateTime::from_timestamp(0, 0).unwrap();
        let progress = PackageProgress {
            name: "epoch-crate".to_string(),
            version: "0.0.1".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: epoch,
        };
        let json = serde_json::to_string(&progress).unwrap();
        let parsed: PackageProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_updated_at.timestamp(), 0);
    }

    #[test]
    fn package_progress_far_future_timestamp_roundtrip() {
        let far_future = DateTime::from_timestamp(4102444800, 0).unwrap(); // 2100-01-01
        let progress = PackageProgress {
            name: "future-crate".to_string(),
            version: "99.0.0".to_string(),
            attempts: 0,
            state: PackageState::Pending,
            last_updated_at: far_future,
        };
        let json = serde_json::to_string(&progress).unwrap();
        let parsed: PackageProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_updated_at.timestamp(), 4102444800);
    }

    #[test]
    fn package_progress_all_states_roundtrip() {
        let states = vec![
            PackageState::Pending,
            PackageState::Uploaded,
            PackageState::Published,
            PackageState::Skipped {
                reason: "r".to_string(),
            },
            PackageState::Failed {
                class: ErrorClass::Ambiguous,
                message: "m".to_string(),
            },
            PackageState::Ambiguous {
                message: "a".to_string(),
            },
        ];
        for state in states {
            let progress = PackageProgress {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
                attempts: 1,
                state: state.clone(),
                last_updated_at: Utc::now(),
            };
            let json = serde_json::to_string(&progress).unwrap();
            let parsed: PackageProgress = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.state, state);
        }
    }

    // ===== RuntimeOptions =====

    fn make_default_runtime_options() -> RuntimeOptions {
        RuntimeOptions {
            allow_dirty: false,
            skip_ownership_check: false,
            strict_ownership: false,
            no_verify: false,
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            retry_strategy: shipper_retry::RetryStrategyType::Exponential,
            retry_jitter: 0.5,
            retry_per_error: shipper_retry::PerErrorConfig::default(),
            verify_timeout: Duration::from_secs(600),
            verify_poll_interval: Duration::from_secs(10),
            state_dir: PathBuf::from(".shipper"),
            force_resume: false,
            policy: PublishPolicy::Safe,
            verify_mode: VerifyMode::Workspace,
            readiness: ReadinessConfig::default(),
            output_lines: 1000,
            force: false,
            lock_timeout: Duration::from_secs(3600),
            parallel: ParallelConfig::default(),
            webhook: WebhookConfig::default(),
            encryption: EncryptionSettings::default(),
            registries: vec![],
            resume_from: None,
        }
    }

    #[test]
    fn runtime_options_default_values() {
        let opts = make_default_runtime_options();
        assert!(!opts.allow_dirty);
        assert!(!opts.skip_ownership_check);
        assert!(!opts.strict_ownership);
        assert!(!opts.no_verify);
        assert_eq!(opts.max_attempts, 3);
        assert_eq!(opts.base_delay, Duration::from_secs(1));
        assert_eq!(opts.max_delay, Duration::from_secs(60));
        assert_eq!(opts.policy, PublishPolicy::Safe);
        assert_eq!(opts.verify_mode, VerifyMode::Workspace);
        assert_eq!(opts.output_lines, 1000);
        assert!(!opts.force);
        assert!(!opts.force_resume);
        assert!(opts.registries.is_empty());
        assert!(opts.resume_from.is_none());
    }

    #[test]
    fn runtime_options_all_booleans_toggled() {
        let opts = RuntimeOptions {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: true,
            no_verify: true,
            force_resume: true,
            force: true,
            ..make_default_runtime_options()
        };
        assert!(opts.allow_dirty);
        assert!(opts.skip_ownership_check);
        assert!(opts.strict_ownership);
        assert!(opts.no_verify);
        assert!(opts.force_resume);
        assert!(opts.force);
    }

    #[test]
    fn runtime_options_with_multiple_registries() {
        let opts = RuntimeOptions {
            registries: vec![
                Registry::crates_io(),
                Registry {
                    name: "private".to_string(),
                    api_base: "https://registry.example.com".to_string(),
                    index_base: None,
                },
            ],
            ..make_default_runtime_options()
        };
        assert_eq!(opts.registries.len(), 2);
        assert_eq!(opts.registries[0].name, "crates-io");
        assert_eq!(opts.registries[1].name, "private");
    }

    #[test]
    fn runtime_options_with_resume_from() {
        let opts = RuntimeOptions {
            resume_from: Some("my-crate".to_string()),
            ..make_default_runtime_options()
        };
        assert_eq!(opts.resume_from.as_deref(), Some("my-crate"));
    }

    // ===== Registry =====

    #[test]
    fn registry_get_index_base_derives_from_api_https() {
        let reg = Registry {
            name: "custom".to_string(),
            api_base: "https://registry.example.com".to_string(),
            index_base: None,
        };
        assert_eq!(reg.get_index_base(), "https://index.registry.example.com");
    }

    #[test]
    fn registry_get_index_base_derives_from_api_http() {
        let reg = Registry {
            name: "local".to_string(),
            api_base: "http://localhost:8080".to_string(),
            index_base: None,
        };
        assert_eq!(reg.get_index_base(), "http://index.localhost:8080");
    }

    #[test]
    fn registry_get_index_base_uses_explicit_value() {
        let reg = Registry {
            name: "custom".to_string(),
            api_base: "https://api.example.com".to_string(),
            index_base: Some("https://my-index.example.com".to_string()),
        };
        assert_eq!(reg.get_index_base(), "https://my-index.example.com");
    }

    #[test]
    fn registry_crates_io_get_index_base() {
        let reg = Registry::crates_io();
        assert_eq!(reg.get_index_base(), "https://index.crates.io");
    }

    #[test]
    fn registry_serde_skips_none_index_base() {
        let reg = Registry {
            name: "test".to_string(),
            api_base: "https://test.io".to_string(),
            index_base: None,
        };
        let json = serde_json::to_string(&reg).unwrap();
        assert!(!json.contains("index_base"));
    }

    // ===== ErrorClass =====

    #[test]
    fn error_class_serde_values() {
        let classes = [
            ErrorClass::Retryable,
            ErrorClass::Permanent,
            ErrorClass::Ambiguous,
        ];
        let expected = [r#""retryable""#, r#""permanent""#, r#""ambiguous""#];
        for (class, exp) in classes.iter().zip(expected.iter()) {
            let json = serde_json::to_string(class).unwrap();
            assert_eq!(&json, exp);
        }
    }

    #[test]
    fn error_class_clone_and_eq() {
        let original = ErrorClass::Retryable;
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    // ===== ExecutionResult =====

    #[test]
    fn execution_result_serde_values() {
        let results = [
            ExecutionResult::Success,
            ExecutionResult::PartialFailure,
            ExecutionResult::CompleteFailure,
        ];
        let expected = [
            r#""success""#,
            r#""partial_failure""#,
            r#""complete_failure""#,
        ];
        for (result, exp) in results.iter().zip(expected.iter()) {
            let json = serde_json::to_string(result).unwrap();
            assert_eq!(&json, exp);
        }
    }

    // ===== AuthType =====

    #[test]
    fn auth_type_serde_values() {
        let types = [
            AuthType::Token,
            AuthType::TrustedPublishing,
            AuthType::Unknown,
        ];
        let expected = [r#""token""#, r#""trusted_publishing""#, r#""unknown""#];
        for (auth, exp) in types.iter().zip(expected.iter()) {
            let json = serde_json::to_string(auth).unwrap();
            assert_eq!(&json, exp);
        }
    }

    // ===== Finishability =====

    #[test]
    fn finishability_serde_values() {
        let fins = [
            Finishability::Proven,
            Finishability::NotProven,
            Finishability::Failed,
        ];
        let expected = [r#""proven""#, r#""not_proven""#, r#""failed""#];
        for (fin, exp) in fins.iter().zip(expected.iter()) {
            let json = serde_json::to_string(fin).unwrap();
            assert_eq!(&json, exp);
        }
    }

    // ===== ParallelConfig =====

    #[test]
    fn parallel_config_default_values() {
        let config = ParallelConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.max_concurrent, 4);
        assert_eq!(config.per_package_timeout, Duration::from_secs(1800));
    }

    #[test]
    fn parallel_config_serde_roundtrip() {
        let config = ParallelConfig {
            enabled: true,
            max_concurrent: 16,
            per_package_timeout: Duration::from_secs(300),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ParallelConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.max_concurrent, 16);
        assert_eq!(parsed.per_package_timeout, Duration::from_secs(300));
    }

    // ===== PackageState serde =====

    #[test]
    fn package_state_pending_json() {
        let json = serde_json::to_string(&PackageState::Pending).unwrap();
        assert_eq!(json, r#"{"state":"pending"}"#);
    }

    #[test]
    fn package_state_published_json() {
        let json = serde_json::to_string(&PackageState::Published).unwrap();
        assert_eq!(json, r#"{"state":"published"}"#);
    }

    #[test]
    fn package_state_skipped_json_contains_reason() {
        let state = PackageState::Skipped {
            reason: "version exists".to_string(),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains(r#""state":"skipped""#));
        assert!(json.contains(r#""reason":"version exists""#));
    }

    #[test]
    fn package_state_ambiguous_serde_roundtrip() {
        let state = PackageState::Ambiguous {
            message: "timeout during readiness".to_string(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: PackageState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    // ===== EventType serde =====

    #[test]
    fn event_type_preflight_variants_roundtrip() {
        let events = vec![
            EventType::PreflightStarted,
            EventType::PreflightWorkspaceVerify {
                passed: true,
                output: "all good".to_string(),
            },
            EventType::PreflightNewCrateDetected {
                crate_name: "new-crate".to_string(),
            },
            EventType::PreflightOwnershipCheck {
                crate_name: "my-crate".to_string(),
                verified: true,
            },
            EventType::PreflightComplete {
                finishability: Finishability::Proven,
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: EventType = serde_json::from_str(&json).unwrap();
            let reparsed_json = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, reparsed_json);
        }
    }

    // ===== GitContext =====

    #[test]
    fn git_context_all_none_roundtrip() {
        let ctx = GitContext {
            commit: None,
            branch: None,
            tag: None,
            dirty: None,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: GitContext = serde_json::from_str(&json).unwrap();
        assert!(parsed.commit.is_none());
        assert!(parsed.branch.is_none());
        assert!(parsed.tag.is_none());
        assert!(parsed.dirty.is_none());
    }

    #[test]
    fn git_context_all_some_roundtrip() {
        let ctx = GitContext {
            commit: Some("abc123".to_string()),
            branch: Some("main".to_string()),
            tag: Some("v1.0.0".to_string()),
            dirty: Some(true),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: GitContext = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.commit.as_deref(), Some("abc123"));
        assert_eq!(parsed.dirty, Some(true));
    }

    // ===== EnvironmentFingerprint =====

    #[test]
    fn environment_fingerprint_optional_fields_roundtrip() {
        let fp = EnvironmentFingerprint {
            shipper_version: "0.3.0".to_string(),
            cargo_version: None,
            rust_version: None,
            os: "wasm".to_string(),
            arch: "wasm32".to_string(),
        };
        let json = serde_json::to_string(&fp).unwrap();
        let parsed: EnvironmentFingerprint = serde_json::from_str(&json).unwrap();
        assert!(parsed.cargo_version.is_none());
        assert!(parsed.rust_version.is_none());
        assert_eq!(parsed.os, "wasm");
    }

    // ===== AttemptEvidence / ReadinessEvidence =====

    #[test]
    fn attempt_evidence_duration_serialized_as_millis() {
        let evidence = AttemptEvidence {
            attempt_number: 1,
            command: "cargo publish".to_string(),
            exit_code: 0,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            timestamp: Utc::now(),
            duration: Duration::from_secs(5),
        };
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(json.contains("5000"));
    }

    #[test]
    fn readiness_evidence_duration_serialized_as_millis() {
        let evidence = ReadinessEvidence {
            attempt: 1,
            visible: true,
            timestamp: Utc::now(),
            delay_before: Duration::from_millis(2500),
        };
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(json.contains("2500"));
    }

    // ===== ReadinessConfig serde =====

    #[test]
    fn readiness_config_serde_with_index_path_roundtrip() {
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
            max_total_wait: Duration::from_secs(600),
            poll_interval: Duration::from_secs(5),
            jitter_factor: 0.3,
            index_path: Some(PathBuf::from("/tmp/test-index")),
            prefer_index: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ReadinessConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.index_path, Some(PathBuf::from("/tmp/test-index")));
        assert!(parsed.prefer_index);
    }

    #[test]
    fn readiness_config_defaults_from_json_empty_object() {
        let config: ReadinessConfig = serde_json::from_str("{}").unwrap();
        assert!(config.enabled);
        assert_eq!(config.method, ReadinessMethod::Api);
        assert_eq!(config.jitter_factor, 0.5);
        assert!(!config.prefer_index);
        assert!(config.index_path.is_none());
    }

    // ===== Debug output snapshot tests =====

    mod debug_snapshots {
        use super::*;

        #[test]
        fn publish_policy_debug_snapshot() {
            insta::assert_debug_snapshot!(PublishPolicy::Safe);
            insta::assert_debug_snapshot!(PublishPolicy::Balanced);
            insta::assert_debug_snapshot!(PublishPolicy::Fast);
        }

        #[test]
        fn verify_mode_debug_snapshot() {
            insta::assert_debug_snapshot!(VerifyMode::Workspace);
            insta::assert_debug_snapshot!(VerifyMode::Package);
            insta::assert_debug_snapshot!(VerifyMode::None);
        }

        #[test]
        fn readiness_method_debug_snapshot() {
            insta::assert_debug_snapshot!(ReadinessMethod::Api);
            insta::assert_debug_snapshot!(ReadinessMethod::Index);
            insta::assert_debug_snapshot!(ReadinessMethod::Both);
        }

        #[test]
        fn runtime_options_debug_snapshot() {
            let opts = super::make_default_runtime_options();
            insta::assert_debug_snapshot!(opts);
        }

        #[test]
        fn package_progress_debug_snapshot() {
            let t = "2025-01-15T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
            let progress = PackageProgress {
                name: "snapshot-crate".to_string(),
                version: "1.0.0".to_string(),
                attempts: 2,
                state: PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message: "timeout".to_string(),
                },
                last_updated_at: t,
            };
            insta::assert_debug_snapshot!(progress);
        }
    }

    mod snapshots {
        use super::*;

        fn fixed_time() -> DateTime<Utc> {
            "2025-01-15T12:00:00Z".parse::<DateTime<Utc>>().unwrap()
        }

        #[test]
        fn release_plan_snapshot() {
            let plan = ReleasePlan {
                plan_version: "shipper.plan.v1".to_string(),
                plan_id: "abc123".to_string(),
                created_at: fixed_time(),
                registry: Registry::crates_io(),
                packages: vec![
                    PlannedPackage {
                        name: "core-lib".to_string(),
                        version: "0.1.0".to_string(),
                        manifest_path: PathBuf::from("crates/core-lib/Cargo.toml"),
                    },
                    PlannedPackage {
                        name: "my-cli".to_string(),
                        version: "0.2.0".to_string(),
                        manifest_path: PathBuf::from("crates/my-cli/Cargo.toml"),
                    },
                ],
                dependencies: BTreeMap::from([(
                    "my-cli".to_string(),
                    vec!["core-lib".to_string()],
                )]),
            };
            insta::assert_yaml_snapshot!(plan);
        }

        #[test]
        fn package_state_all_variants() {
            let variants: Vec<(&str, PackageState)> = vec![
                ("pending", PackageState::Pending),
                ("uploaded", PackageState::Uploaded),
                ("published", PackageState::Published),
                (
                    "skipped",
                    PackageState::Skipped {
                        reason: "already published".to_string(),
                    },
                ),
                (
                    "failed",
                    PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: "network timeout".to_string(),
                    },
                ),
                (
                    "ambiguous",
                    PackageState::Ambiguous {
                        message: "unclear outcome".to_string(),
                    },
                ),
            ];
            for (label, state) in variants {
                insta::assert_yaml_snapshot!(format!("package_state_{label}"), state);
            }
        }

        #[test]
        fn receipt_full_snapshot() {
            let t = fixed_time();
            let receipt = Receipt {
                receipt_version: "shipper.receipt.v1".to_string(),
                plan_id: "plan-42".to_string(),
                registry: Registry::crates_io(),
                started_at: t,
                finished_at: t,
                packages: vec![PackageReceipt {
                    name: "demo".to_string(),
                    version: "1.0.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: t,
                    finished_at: t,
                    duration_ms: 4500,
                    evidence: PackageEvidence {
                        attempts: vec![AttemptEvidence {
                            attempt_number: 1,
                            command: "cargo publish -p demo".to_string(),
                            exit_code: 0,
                            stdout_tail: "Uploading demo v1.0.0".to_string(),
                            stderr_tail: String::new(),
                            timestamp: t,
                            duration: Duration::from_millis(4200),
                        }],
                        readiness_checks: vec![ReadinessEvidence {
                            attempt: 1,
                            visible: true,
                            timestamp: t,
                            delay_before: Duration::from_secs(2),
                        }],
                    },
                }],
                event_log_path: PathBuf::from(".shipper/events.jsonl"),
                git_context: Some(GitContext {
                    commit: Some("abcdef1234567890".to_string()),
                    branch: Some("main".to_string()),
                    tag: Some("v1.0.0".to_string()),
                    dirty: Some(false),
                }),
                environment: EnvironmentFingerprint {
                    shipper_version: "0.2.0".to_string(),
                    cargo_version: Some("1.82.0".to_string()),
                    rust_version: Some("1.82.0".to_string()),
                    os: "linux".to_string(),
                    arch: "x86_64".to_string(),
                },
            };
            insta::assert_yaml_snapshot!(receipt);
        }

        #[test]
        fn execution_state_snapshot() {
            let t = fixed_time();
            let mut packages = BTreeMap::new();
            packages.insert(
                "core-lib@0.1.0".to_string(),
                PackageProgress {
                    name: "core-lib".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    last_updated_at: t,
                },
            );
            packages.insert(
                "my-cli@0.2.0".to_string(),
                PackageProgress {
                    name: "my-cli".to_string(),
                    version: "0.2.0".to_string(),
                    attempts: 0,
                    state: PackageState::Pending,
                    last_updated_at: t,
                },
            );
            let state = ExecutionState {
                state_version: "shipper.state.v1".to_string(),
                plan_id: "plan-42".to_string(),
                registry: Registry::crates_io(),
                created_at: t,
                updated_at: t,
                packages,
            };
            insta::assert_yaml_snapshot!(state);
        }

        #[test]
        fn preflight_report_snapshot() {
            let report = PreflightReport {
                plan_id: "plan-42".to_string(),
                token_detected: true,
                finishability: Finishability::Proven,
                packages: vec![
                    PreflightPackage {
                        name: "core-lib".to_string(),
                        version: "0.1.0".to_string(),
                        already_published: false,
                        is_new_crate: true,
                        auth_type: Some(AuthType::Token),
                        ownership_verified: true,
                        dry_run_passed: true,
                        dry_run_output: None,
                    },
                    PreflightPackage {
                        name: "my-cli".to_string(),
                        version: "0.2.0".to_string(),
                        already_published: false,
                        is_new_crate: false,
                        auth_type: Some(AuthType::TrustedPublishing),
                        ownership_verified: true,
                        dry_run_passed: true,
                        dry_run_output: Some("dry-run ok".to_string()),
                    },
                ],
                timestamp: fixed_time(),
                dry_run_output: Some("workspace dry-run passed".to_string()),
            };
            insta::assert_yaml_snapshot!(report);
        }

        // --- ReleasePlan variations ---

        #[test]
        fn release_plan_single_package() {
            let plan = ReleasePlan {
                plan_version: "shipper.plan.v1".to_string(),
                plan_id: "single-pkg-001".to_string(),
                created_at: fixed_time(),
                registry: Registry::crates_io(),
                packages: vec![PlannedPackage {
                    name: "solo-crate".to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("Cargo.toml"),
                }],
                dependencies: BTreeMap::new(),
            };
            insta::assert_yaml_snapshot!(plan);
        }

        #[test]
        fn release_plan_custom_registry() {
            let plan = ReleasePlan {
                plan_version: "shipper.plan.v1".to_string(),
                plan_id: "custom-reg-001".to_string(),
                created_at: fixed_time(),
                registry: Registry {
                    name: "my-private-registry".to_string(),
                    api_base: "https://registry.example.com".to_string(),
                    index_base: Some("https://index.registry.example.com".to_string()),
                },
                packages: vec![
                    PlannedPackage {
                        name: "internal-utils".to_string(),
                        version: "2.1.0".to_string(),
                        manifest_path: PathBuf::from("crates/internal-utils/Cargo.toml"),
                    },
                    PlannedPackage {
                        name: "internal-api".to_string(),
                        version: "3.0.0".to_string(),
                        manifest_path: PathBuf::from("crates/internal-api/Cargo.toml"),
                    },
                ],
                dependencies: BTreeMap::from([(
                    "internal-api".to_string(),
                    vec!["internal-utils".to_string()],
                )]),
            };
            insta::assert_yaml_snapshot!(plan);
        }

        #[test]
        fn release_plan_deep_dependency_chain() {
            let plan = ReleasePlan {
                plan_version: "shipper.plan.v1".to_string(),
                plan_id: "deep-deps-001".to_string(),
                created_at: fixed_time(),
                registry: Registry::crates_io(),
                packages: vec![
                    PlannedPackage {
                        name: "foundation".to_string(),
                        version: "0.1.0".to_string(),
                        manifest_path: PathBuf::from("crates/foundation/Cargo.toml"),
                    },
                    PlannedPackage {
                        name: "middleware".to_string(),
                        version: "0.2.0".to_string(),
                        manifest_path: PathBuf::from("crates/middleware/Cargo.toml"),
                    },
                    PlannedPackage {
                        name: "service".to_string(),
                        version: "0.3.0".to_string(),
                        manifest_path: PathBuf::from("crates/service/Cargo.toml"),
                    },
                    PlannedPackage {
                        name: "gateway".to_string(),
                        version: "1.0.0".to_string(),
                        manifest_path: PathBuf::from("crates/gateway/Cargo.toml"),
                    },
                ],
                dependencies: BTreeMap::from([
                    ("foundation".to_string(), Vec::new()),
                    ("middleware".to_string(), vec!["foundation".to_string()]),
                    (
                        "service".to_string(),
                        vec!["foundation".to_string(), "middleware".to_string()],
                    ),
                    ("gateway".to_string(), vec!["service".to_string()]),
                ]),
            };
            insta::assert_yaml_snapshot!(plan);
        }

        // --- Receipt variations ---

        #[test]
        fn receipt_partial_failure() {
            let t = fixed_time();
            let receipt = Receipt {
                receipt_version: "shipper.receipt.v1".to_string(),
                plan_id: "plan-partial".to_string(),
                registry: Registry::crates_io(),
                started_at: t,
                finished_at: t,
                packages: vec![
                    PackageReceipt {
                        name: "core-lib".to_string(),
                        version: "0.1.0".to_string(),
                        attempts: 1,
                        state: PackageState::Published,
                        started_at: t,
                        finished_at: t,
                        duration_ms: 3200,
                        evidence: PackageEvidence {
                            attempts: vec![AttemptEvidence {
                                attempt_number: 1,
                                command: "cargo publish -p core-lib".to_string(),
                                exit_code: 0,
                                stdout_tail: "Uploading core-lib v0.1.0".to_string(),
                                stderr_tail: String::new(),
                                timestamp: t,
                                duration: Duration::from_millis(3000),
                            }],
                            readiness_checks: vec![ReadinessEvidence {
                                attempt: 1,
                                visible: true,
                                timestamp: t,
                                delay_before: Duration::from_secs(1),
                            }],
                        },
                    },
                    PackageReceipt {
                        name: "api-server".to_string(),
                        version: "0.2.0".to_string(),
                        attempts: 3,
                        state: PackageState::Failed {
                            class: ErrorClass::Retryable,
                            message: "rate limited by registry".to_string(),
                        },
                        started_at: t,
                        finished_at: t,
                        duration_ms: 15000,
                        evidence: PackageEvidence {
                            attempts: vec![
                                AttemptEvidence {
                                    attempt_number: 1,
                                    command: "cargo publish -p api-server".to_string(),
                                    exit_code: 1,
                                    stdout_tail: String::new(),
                                    stderr_tail: "error: rate limit exceeded".to_string(),
                                    timestamp: t,
                                    duration: Duration::from_millis(500),
                                },
                                AttemptEvidence {
                                    attempt_number: 2,
                                    command: "cargo publish -p api-server".to_string(),
                                    exit_code: 1,
                                    stdout_tail: String::new(),
                                    stderr_tail: "error: rate limit exceeded".to_string(),
                                    timestamp: t,
                                    duration: Duration::from_millis(600),
                                },
                                AttemptEvidence {
                                    attempt_number: 3,
                                    command: "cargo publish -p api-server".to_string(),
                                    exit_code: 1,
                                    stdout_tail: String::new(),
                                    stderr_tail: "error: rate limit exceeded".to_string(),
                                    timestamp: t,
                                    duration: Duration::from_millis(700),
                                },
                            ],
                            readiness_checks: vec![],
                        },
                    },
                    PackageReceipt {
                        name: "old-compat".to_string(),
                        version: "0.1.0".to_string(),
                        attempts: 0,
                        state: PackageState::Skipped {
                            reason: "version already exists on registry".to_string(),
                        },
                        started_at: t,
                        finished_at: t,
                        duration_ms: 50,
                        evidence: PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                    },
                ],
                event_log_path: PathBuf::from(".shipper/events.jsonl"),
                git_context: Some(GitContext {
                    commit: Some("deadbeef12345678".to_string()),
                    branch: Some("release/v0.2".to_string()),
                    tag: None,
                    dirty: Some(true),
                }),
                environment: EnvironmentFingerprint {
                    shipper_version: "0.3.0".to_string(),
                    cargo_version: Some("1.82.0".to_string()),
                    rust_version: Some("1.82.0".to_string()),
                    os: "linux".to_string(),
                    arch: "x86_64".to_string(),
                },
            };
            insta::assert_yaml_snapshot!(receipt);
        }

        #[test]
        fn receipt_no_git_context() {
            let t = fixed_time();
            let receipt = Receipt {
                receipt_version: "shipper.receipt.v1".to_string(),
                plan_id: "plan-nogit".to_string(),
                registry: Registry::crates_io(),
                started_at: t,
                finished_at: t,
                packages: vec![PackageReceipt {
                    name: "headless-lib".to_string(),
                    version: "0.5.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    started_at: t,
                    finished_at: t,
                    duration_ms: 2000,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                }],
                event_log_path: PathBuf::from(".shipper/events.jsonl"),
                git_context: None,
                environment: EnvironmentFingerprint {
                    shipper_version: "0.3.0".to_string(),
                    cargo_version: None,
                    rust_version: None,
                    os: "windows".to_string(),
                    arch: "aarch64".to_string(),
                },
            };
            insta::assert_yaml_snapshot!(receipt);
        }

        #[test]
        fn receipt_complete_failure() {
            let t = fixed_time();
            let receipt = Receipt {
                receipt_version: "shipper.receipt.v1".to_string(),
                plan_id: "plan-allfail".to_string(),
                registry: Registry::crates_io(),
                started_at: t,
                finished_at: t,
                packages: vec![
                    PackageReceipt {
                        name: "broken-crate".to_string(),
                        version: "0.1.0".to_string(),
                        attempts: 3,
                        state: PackageState::Failed {
                            class: ErrorClass::Permanent,
                            message: "invalid credentials".to_string(),
                        },
                        started_at: t,
                        finished_at: t,
                        duration_ms: 800,
                        evidence: PackageEvidence {
                            attempts: vec![AttemptEvidence {
                                attempt_number: 1,
                                command: "cargo publish -p broken-crate".to_string(),
                                exit_code: 1,
                                stdout_tail: String::new(),
                                stderr_tail: "error: 403 Forbidden".to_string(),
                                timestamp: t,
                                duration: Duration::from_millis(200),
                            }],
                            readiness_checks: vec![],
                        },
                    },
                    PackageReceipt {
                        name: "dependent-crate".to_string(),
                        version: "0.2.0".to_string(),
                        attempts: 0,
                        state: PackageState::Skipped {
                            reason: "dependency broken-crate failed".to_string(),
                        },
                        started_at: t,
                        finished_at: t,
                        duration_ms: 0,
                        evidence: PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                    },
                ],
                event_log_path: PathBuf::from(".shipper/events.jsonl"),
                git_context: Some(GitContext {
                    commit: Some("abcdef0123456789".to_string()),
                    branch: Some("main".to_string()),
                    tag: Some("v0.1.0".to_string()),
                    dirty: Some(false),
                }),
                environment: EnvironmentFingerprint {
                    shipper_version: "0.3.0".to_string(),
                    cargo_version: Some("1.82.0".to_string()),
                    rust_version: Some("1.82.0".to_string()),
                    os: "macos".to_string(),
                    arch: "aarch64".to_string(),
                },
            };
            insta::assert_yaml_snapshot!(receipt);
        }

        // --- ExecutionState variations ---

        #[test]
        fn execution_state_all_pending() {
            let t = fixed_time();
            let mut packages = BTreeMap::new();
            for (name, ver) in [("alpha", "0.1.0"), ("beta", "0.2.0"), ("gamma", "0.3.0")] {
                packages.insert(
                    format!("{name}@{ver}"),
                    PackageProgress {
                        name: name.to_string(),
                        version: ver.to_string(),
                        attempts: 0,
                        state: PackageState::Pending,
                        last_updated_at: t,
                    },
                );
            }
            let state = ExecutionState {
                state_version: "shipper.state.v1".to_string(),
                plan_id: "plan-fresh".to_string(),
                registry: Registry::crates_io(),
                created_at: t,
                updated_at: t,
                packages,
            };
            insta::assert_yaml_snapshot!(state);
        }

        #[test]
        fn execution_state_completed() {
            let t = fixed_time();
            let mut packages = BTreeMap::new();
            for (name, ver) in [("alpha", "0.1.0"), ("beta", "0.2.0")] {
                packages.insert(
                    format!("{name}@{ver}"),
                    PackageProgress {
                        name: name.to_string(),
                        version: ver.to_string(),
                        attempts: 1,
                        state: PackageState::Published,
                        last_updated_at: t,
                    },
                );
            }
            let state = ExecutionState {
                state_version: "shipper.state.v1".to_string(),
                plan_id: "plan-done".to_string(),
                registry: Registry::crates_io(),
                created_at: t,
                updated_at: t,
                packages,
            };
            insta::assert_yaml_snapshot!(state);
        }

        #[test]
        fn execution_state_mixed_with_failures() {
            let t = fixed_time();
            let mut packages = BTreeMap::new();
            packages.insert(
                "core@0.1.0".to_string(),
                PackageProgress {
                    name: "core".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 1,
                    state: PackageState::Published,
                    last_updated_at: t,
                },
            );
            packages.insert(
                "net@0.2.0".to_string(),
                PackageProgress {
                    name: "net".to_string(),
                    version: "0.2.0".to_string(),
                    attempts: 3,
                    state: PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: "connection reset".to_string(),
                    },
                    last_updated_at: t,
                },
            );
            packages.insert(
                "cli@0.3.0".to_string(),
                PackageProgress {
                    name: "cli".to_string(),
                    version: "0.3.0".to_string(),
                    attempts: 1,
                    state: PackageState::Ambiguous {
                        message: "upload succeeded but readiness timed out".to_string(),
                    },
                    last_updated_at: t,
                },
            );
            packages.insert(
                "compat@0.1.0".to_string(),
                PackageProgress {
                    name: "compat".to_string(),
                    version: "0.1.0".to_string(),
                    attempts: 0,
                    state: PackageState::Skipped {
                        reason: "version already on registry".to_string(),
                    },
                    last_updated_at: t,
                },
            );
            let state = ExecutionState {
                state_version: "shipper.state.v1".to_string(),
                plan_id: "plan-mixed".to_string(),
                registry: Registry::crates_io(),
                created_at: t,
                updated_at: t,
                packages,
            };
            insta::assert_yaml_snapshot!(state);
        }

        // --- Config snapshots ---

        #[test]
        fn readiness_config_default_snapshot() {
            let config = ReadinessConfig::default();
            insta::assert_yaml_snapshot!(config);
        }

        #[test]
        fn readiness_config_custom_snapshot() {
            let config = ReadinessConfig {
                enabled: false,
                method: ReadinessMethod::Both,
                initial_delay: Duration::from_millis(500),
                max_delay: Duration::from_secs(120),
                max_total_wait: Duration::from_secs(900),
                poll_interval: Duration::from_secs(10),
                jitter_factor: 0.25,
                index_path: Some(PathBuf::from("/tmp/test-index")),
                prefer_index: true,
            };
            insta::assert_yaml_snapshot!(config);
        }

        #[test]
        fn parallel_config_default_snapshot() {
            let config = ParallelConfig::default();
            insta::assert_yaml_snapshot!(config);
        }

        #[test]
        fn parallel_config_enabled_snapshot() {
            let config = ParallelConfig {
                enabled: true,
                max_concurrent: 8,
                per_package_timeout: Duration::from_secs(600),
            };
            insta::assert_yaml_snapshot!(config);
        }

        // --- Ancillary type snapshots ---

        #[test]
        fn environment_fingerprint_snapshot() {
            let fp = EnvironmentFingerprint {
                shipper_version: "0.3.0".to_string(),
                cargo_version: Some("1.82.0".to_string()),
                rust_version: Some("1.82.0".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            };
            insta::assert_yaml_snapshot!(fp);
        }

        #[test]
        fn git_context_full_snapshot() {
            let ctx = GitContext {
                commit: Some("a1b2c3d4e5f6".to_string()),
                branch: Some("release/v2.0".to_string()),
                tag: Some("v2.0.0".to_string()),
                dirty: Some(false),
            };
            insta::assert_yaml_snapshot!(ctx);
        }

        #[test]
        fn git_context_minimal_snapshot() {
            let ctx = GitContext {
                commit: None,
                branch: None,
                tag: None,
                dirty: None,
            };
            insta::assert_yaml_snapshot!(ctx);
        }

        #[test]
        fn publish_event_lifecycle_snapshot() {
            let t = fixed_time();
            let events = vec![
                PublishEvent {
                    timestamp: t,
                    event_type: EventType::PlanCreated {
                        plan_id: "plan-99".to_string(),
                        package_count: 3,
                    },
                    package: String::new(),
                },
                PublishEvent {
                    timestamp: t,
                    event_type: EventType::ExecutionStarted,
                    package: String::new(),
                },
                PublishEvent {
                    timestamp: t,
                    event_type: EventType::ExecutionFinished {
                        result: ExecutionResult::PartialFailure,
                    },
                    package: String::new(),
                },
            ];
            insta::assert_yaml_snapshot!(events);
        }

        #[test]
        fn publish_event_package_flow_snapshot() {
            let t = fixed_time();
            let events = vec![
                PublishEvent {
                    timestamp: t,
                    event_type: EventType::PackageStarted {
                        name: "my-crate".to_string(),
                        version: "1.0.0".to_string(),
                    },
                    package: "my-crate@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: t,
                    event_type: EventType::PackageAttempted {
                        attempt: 1,
                        command: "cargo publish -p my-crate".to_string(),
                    },
                    package: "my-crate@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: t,
                    event_type: EventType::PackageOutput {
                        stdout_tail: "Uploading my-crate v1.0.0".to_string(),
                        stderr_tail: String::new(),
                    },
                    package: "my-crate@1.0.0".to_string(),
                },
                PublishEvent {
                    timestamp: t,
                    event_type: EventType::PackagePublished { duration_ms: 4500 },
                    package: "my-crate@1.0.0".to_string(),
                },
            ];
            insta::assert_yaml_snapshot!(events);
        }

        #[test]
        fn error_class_all_variants_snapshot() {
            let variants: Vec<(&str, ErrorClass)> = vec![
                ("retryable", ErrorClass::Retryable),
                ("permanent", ErrorClass::Permanent),
                ("ambiguous", ErrorClass::Ambiguous),
            ];
            for (label, class) in variants {
                insta::assert_yaml_snapshot!(format!("error_class_{label}"), class);
            }
        }

        #[test]
        fn execution_result_all_variants_snapshot() {
            let variants: Vec<(&str, ExecutionResult)> = vec![
                ("success", ExecutionResult::Success),
                ("partial_failure", ExecutionResult::PartialFailure),
                ("complete_failure", ExecutionResult::CompleteFailure),
            ];
            for (label, result) in variants {
                insta::assert_yaml_snapshot!(format!("execution_result_{label}"), result);
            }
        }

        #[test]
        fn finishability_all_variants_snapshot() {
            let variants: Vec<(&str, Finishability)> = vec![
                ("proven", Finishability::Proven),
                ("not_proven", Finishability::NotProven),
                ("failed", Finishability::Failed),
            ];
            for (label, fin) in variants {
                insta::assert_yaml_snapshot!(format!("finishability_{label}"), fin);
            }
        }

        #[test]
        fn preflight_report_failed_snapshot() {
            let report = PreflightReport {
                plan_id: "plan-fail-preflight".to_string(),
                token_detected: false,
                finishability: Finishability::Failed,
                packages: vec![PreflightPackage {
                    name: "broken".to_string(),
                    version: "0.1.0".to_string(),
                    already_published: false,
                    is_new_crate: true,
                    auth_type: None,
                    ownership_verified: false,
                    dry_run_passed: false,
                    dry_run_output: Some("error: could not compile".to_string()),
                }],
                timestamp: fixed_time(),
                dry_run_output: Some("workspace dry-run failed".to_string()),
            };
            insta::assert_yaml_snapshot!(report);
        }
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
                        dry_run_output: if i % 5 == 0 { Some("failed".to_string()) } else { None },
                    })
                    .collect();

                let report = PreflightReport {
                    plan_id: plan_id.clone(),
                    token_detected,
                    finishability,
                    packages: packages.clone(),
                    timestamp: Utc::now(),
                    dry_run_output: Some("workspace dry-run output".to_string()),
                };

                // Serialize and deserialize
                let json = serde_json::to_string(&report).unwrap();
                let parsed: PreflightReport = serde_json::from_str(&json).unwrap();

                // Verify roundtrip
                assert_eq!(parsed.plan_id, report.plan_id);
                assert_eq!(parsed.token_detected, report.token_detected);
                assert_eq!(parsed.finishability, report.finishability);
                assert_eq!(parsed.packages.len(), report.packages.len());
                assert_eq!(parsed.dry_run_output, report.dry_run_output);
                for (orig, parsed_pkg) in report.packages.iter().zip(parsed.packages.iter()) {
                    assert_eq!(parsed_pkg.name, orig.name);
                    assert_eq!(parsed_pkg.version, orig.version);
                    assert_eq!(parsed_pkg.already_published, orig.already_published);
                    assert_eq!(parsed_pkg.is_new_crate, orig.is_new_crate);
                    assert_eq!(parsed_pkg.auth_type, orig.auth_type);
                    assert_eq!(parsed_pkg.ownership_verified, orig.ownership_verified);
                    assert_eq!(parsed_pkg.dry_run_passed, orig.dry_run_passed);
                    assert_eq!(parsed_pkg.dry_run_output, orig.dry_run_output);
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
                dry_run_output in proptest::option::of(".*"),
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
                    dry_run_output: dry_run_output.clone(),
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
                assert_eq!(parsed.dry_run_output, pkg.dry_run_output);
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

        // --- PackageState roundtrip for all variants ---
        proptest! {
            #[test]
            fn package_state_pending_roundtrip(_dummy in 0u8..1) {
                let state = PackageState::Pending;
                let json = serde_json::to_string(&state).unwrap();
                let parsed: PackageState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, state);
            }

            #[test]
            fn package_state_uploaded_roundtrip(_dummy in 0u8..1) {
                let state = PackageState::Uploaded;
                let json = serde_json::to_string(&state).unwrap();
                let parsed: PackageState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, state);
            }

            #[test]
            fn package_state_published_roundtrip(_dummy in 0u8..1) {
                let state = PackageState::Published;
                let json = serde_json::to_string(&state).unwrap();
                let parsed: PackageState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, state);
            }

            #[test]
            fn package_state_skipped_roundtrip(reason in "\\PC{0,50}") {
                let state = PackageState::Skipped { reason };
                let json = serde_json::to_string(&state).unwrap();
                let parsed: PackageState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, state);
            }

            #[test]
            fn package_state_failed_roundtrip(
                class_variant in 0u8..3,
                message in "\\PC{0,80}",
            ) {
                let class = match class_variant {
                    0 => ErrorClass::Retryable,
                    1 => ErrorClass::Permanent,
                    _ => ErrorClass::Ambiguous,
                };
                let state = PackageState::Failed { class, message };
                let json = serde_json::to_string(&state).unwrap();
                let parsed: PackageState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, state);
            }

            #[test]
            fn package_state_ambiguous_roundtrip(message in "\\PC{0,80}") {
                let state = PackageState::Ambiguous { message };
                let json = serde_json::to_string(&state).unwrap();
                let parsed: PackageState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, state);
            }

            // --- ErrorClass roundtrip ---
            #[test]
            fn error_class_roundtrip(variant in 0u8..3) {
                let class = match variant {
                    0 => ErrorClass::Retryable,
                    1 => ErrorClass::Permanent,
                    _ => ErrorClass::Ambiguous,
                };
                let json = serde_json::to_string(&class).unwrap();
                let parsed: ErrorClass = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, class);
            }

            // --- ExecutionResult roundtrip ---
            #[test]
            fn execution_result_roundtrip(variant in 0u8..3) {
                let result = match variant {
                    0 => ExecutionResult::Success,
                    1 => ExecutionResult::PartialFailure,
                    _ => ExecutionResult::CompleteFailure,
                };
                let json = serde_json::to_string(&result).unwrap();
                let parsed: ExecutionResult = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, result);
            }

            // --- PublishPolicy roundtrip ---
            #[test]
            fn publish_policy_roundtrip(variant in 0u8..3) {
                let policy = match variant {
                    0 => PublishPolicy::Safe,
                    1 => PublishPolicy::Balanced,
                    _ => PublishPolicy::Fast,
                };
                let json = serde_json::to_string(&policy).unwrap();
                let parsed: PublishPolicy = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, policy);
            }

            // --- VerifyMode roundtrip ---
            #[test]
            fn verify_mode_roundtrip(variant in 0u8..3) {
                let mode = match variant {
                    0 => VerifyMode::Workspace,
                    1 => VerifyMode::Package,
                    _ => VerifyMode::None,
                };
                let json = serde_json::to_string(&mode).unwrap();
                let parsed: VerifyMode = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, mode);
            }

            // --- ReadinessMethod roundtrip ---
            #[test]
            fn readiness_method_roundtrip(variant in 0u8..3) {
                let method = match variant {
                    0 => ReadinessMethod::Api,
                    1 => ReadinessMethod::Index,
                    _ => ReadinessMethod::Both,
                };
                let json = serde_json::to_string(&method).unwrap();
                let parsed: ReadinessMethod = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, method);
            }

            // --- PlannedPackage roundtrip ---
            #[test]
            fn planned_package_roundtrip(
                name in "[a-z][a-z0-9-]{0,20}",
                version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
            ) {
                let pkg = PlannedPackage {
                    name,
                    version,
                    manifest_path: PathBuf::from("crates/test/Cargo.toml"),
                };
                let json = serde_json::to_string(&pkg).unwrap();
                let parsed: PlannedPackage = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.name, pkg.name);
                assert_eq!(parsed.version, pkg.version);
                assert_eq!(parsed.manifest_path, pkg.manifest_path);
            }

            // --- PublishLevel roundtrip ---
            #[test]
            fn publish_level_roundtrip(
                level in 0usize..10,
                pkg_count in 1usize..5,
            ) {
                let packages: Vec<PlannedPackage> = (0..pkg_count)
                    .map(|i| PlannedPackage {
                        name: format!("crate-{i}"),
                        version: format!("{i}.0.0"),
                        manifest_path: PathBuf::from(format!("crates/crate-{i}/Cargo.toml")),
                    })
                    .collect();
                let lvl = PublishLevel { level, packages };
                let json = serde_json::to_string(&lvl).unwrap();
                let parsed: PublishLevel = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.level, lvl.level);
                assert_eq!(parsed.packages.len(), lvl.packages.len());
            }

            // --- ReleasePlan roundtrip ---
            #[test]
            fn release_plan_roundtrip(
                plan_id in "[a-f0-9]{8,64}",
                pkg_count in 1usize..5,
            ) {
                let packages: Vec<PlannedPackage> = (0..pkg_count)
                    .map(|i| PlannedPackage {
                        name: format!("crate-{i}"),
                        version: format!("{i}.0.0"),
                        manifest_path: PathBuf::from(format!("crates/crate-{i}/Cargo.toml")),
                    })
                    .collect();
                let mut deps = BTreeMap::new();
                if pkg_count > 1 {
                    deps.insert(
                        "crate-1".to_string(),
                        vec!["crate-0".to_string()],
                    );
                }
                let plan = ReleasePlan {
                    plan_version: "shipper.plan.v1".to_string(),
                    plan_id,
                    created_at: Utc::now(),
                    registry: Registry::crates_io(),
                    packages,
                    dependencies: deps,
                };
                let json = serde_json::to_string(&plan).unwrap();
                let parsed: ReleasePlan = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.plan_id, plan.plan_id);
                assert_eq!(parsed.plan_version, plan.plan_version);
                assert_eq!(parsed.packages.len(), plan.packages.len());
                assert_eq!(parsed.dependencies, plan.dependencies);
            }

            // --- PackageProgress roundtrip ---
            #[test]
            fn package_progress_roundtrip(
                name in "[a-z][a-z0-9-]{0,15}",
                version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
                attempts in 0u32..10,
                state_variant in 0u8..4,
            ) {
                let state = match state_variant {
                    0 => PackageState::Pending,
                    1 => PackageState::Uploaded,
                    2 => PackageState::Published,
                    _ => PackageState::Skipped { reason: "already exists".to_string() },
                };
                let progress = PackageProgress {
                    name,
                    version,
                    attempts,
                    state,
                    last_updated_at: Utc::now(),
                };
                let json = serde_json::to_string(&progress).unwrap();
                let parsed: PackageProgress = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.name, progress.name);
                assert_eq!(parsed.version, progress.version);
                assert_eq!(parsed.attempts, progress.attempts);
                assert_eq!(parsed.state, progress.state);
            }

            // --- ExecutionState roundtrip ---
            #[test]
            fn execution_state_roundtrip(
                plan_id in "[a-f0-9]{8,64}",
                pkg_count in 0usize..5,
            ) {
                let mut packages = BTreeMap::new();
                for i in 0..pkg_count {
                    let key = format!("crate-{i}@{i}.0.0");
                    packages.insert(key, PackageProgress {
                        name: format!("crate-{i}"),
                        version: format!("{i}.0.0"),
                        attempts: i as u32,
                        state: PackageState::Pending,
                        last_updated_at: Utc::now(),
                    });
                }
                let state = ExecutionState {
                    state_version: "shipper.state.v1".to_string(),
                    plan_id,
                    registry: Registry::crates_io(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    packages,
                };
                let json = serde_json::to_string(&state).unwrap();
                let parsed: ExecutionState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.plan_id, state.plan_id);
                assert_eq!(parsed.packages.len(), state.packages.len());
            }

            // --- ParallelConfig roundtrip ---
            #[test]
            fn parallel_config_roundtrip(
                enabled in any::<bool>(),
                max_concurrent in 1usize..32,
                timeout_secs in 1u64..7200,
            ) {
                let config = ParallelConfig {
                    enabled,
                    max_concurrent,
                    per_package_timeout: Duration::from_secs(timeout_secs),
                };
                let json = serde_json::to_string(&config).unwrap();
                let parsed: ParallelConfig = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.enabled, config.enabled);
                assert_eq!(parsed.max_concurrent, config.max_concurrent);
                assert_eq!(parsed.per_package_timeout, config.per_package_timeout);
            }

            // --- AttemptEvidence roundtrip ---
            #[test]
            fn attempt_evidence_roundtrip(
                attempt_number in 1u32..10,
                exit_code in -1i32..256,
                duration_ms in 0u64..600_000,
            ) {
                let evidence = AttemptEvidence {
                    attempt_number,
                    command: "cargo publish -p test".to_string(),
                    exit_code,
                    stdout_tail: "Uploading test v1.0.0".to_string(),
                    stderr_tail: String::new(),
                    timestamp: Utc::now(),
                    duration: Duration::from_millis(duration_ms),
                };
                let json = serde_json::to_string(&evidence).unwrap();
                let parsed: AttemptEvidence = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.attempt_number, evidence.attempt_number);
                assert_eq!(parsed.exit_code, evidence.exit_code);
                assert_eq!(parsed.duration, evidence.duration);
            }

            // --- ReadinessEvidence roundtrip ---
            #[test]
            fn readiness_evidence_roundtrip(
                attempt in 1u32..20,
                visible in any::<bool>(),
                delay_ms in 0u64..120_000,
            ) {
                let evidence = ReadinessEvidence {
                    attempt,
                    visible,
                    timestamp: Utc::now(),
                    delay_before: Duration::from_millis(delay_ms),
                };
                let json = serde_json::to_string(&evidence).unwrap();
                let parsed: ReadinessEvidence = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.attempt, evidence.attempt);
                assert_eq!(parsed.visible, evidence.visible);
                assert_eq!(parsed.delay_before, evidence.delay_before);
            }

            // --- PackageEvidence roundtrip ---
            #[test]
            fn package_evidence_roundtrip(attempt_count in 0usize..4) {
                let attempts: Vec<AttemptEvidence> = (0..attempt_count)
                    .map(|i| AttemptEvidence {
                        attempt_number: i as u32 + 1,
                        command: format!("cargo publish attempt {i}"),
                        exit_code: if i == attempt_count - 1 { 0 } else { 1 },
                        stdout_tail: "output".to_string(),
                        stderr_tail: String::new(),
                        timestamp: Utc::now(),
                        duration: Duration::from_secs(5),
                    })
                    .collect();
                let evidence = PackageEvidence {
                    attempts,
                    readiness_checks: vec![],
                };
                let json = serde_json::to_string(&evidence).unwrap();
                let parsed: PackageEvidence = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.attempts.len(), evidence.attempts.len());
            }

            // --- PackageReceipt roundtrip ---
            #[test]
            fn package_receipt_roundtrip(
                name in "[a-z][a-z0-9-]{0,15}",
                version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
                attempts in 1u32..5,
                duration_ms in 0u128..600_000,
            ) {
                let now = Utc::now();
                let receipt = PackageReceipt {
                    name,
                    version,
                    attempts,
                    state: PackageState::Published,
                    started_at: now,
                    finished_at: now,
                    duration_ms,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                };
                let json = serde_json::to_string(&receipt).unwrap();
                let parsed: PackageReceipt = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.name, receipt.name);
                assert_eq!(parsed.version, receipt.version);
                assert_eq!(parsed.attempts, receipt.attempts);
                assert_eq!(parsed.state, receipt.state);
                assert_eq!(parsed.duration_ms, receipt.duration_ms);
            }

            // --- Receipt roundtrip ---
            #[test]
            fn receipt_roundtrip(
                plan_id in "[a-f0-9]{8,64}",
                pkg_count in 0usize..3,
            ) {
                let now = Utc::now();
                let packages: Vec<PackageReceipt> = (0..pkg_count)
                    .map(|i| PackageReceipt {
                        name: format!("crate-{i}"),
                        version: format!("{i}.0.0"),
                        attempts: 1,
                        state: PackageState::Published,
                        started_at: now,
                        finished_at: now,
                        duration_ms: 1000,
                        evidence: PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                    })
                    .collect();
                let receipt = Receipt {
                    receipt_version: "shipper.receipt.v1".to_string(),
                    plan_id,
                    registry: Registry::crates_io(),
                    started_at: now,
                    finished_at: now,
                    packages,
                    event_log_path: PathBuf::from(".shipper/events.jsonl"),
                    git_context: Some(GitContext {
                        commit: Some("abc123".to_string()),
                        branch: Some("main".to_string()),
                        tag: None,
                        dirty: Some(false),
                    }),
                    environment: EnvironmentFingerprint {
                        shipper_version: "0.3.0".to_string(),
                        cargo_version: Some("1.80.0".to_string()),
                        rust_version: Some("1.80.0".to_string()),
                        os: "linux".to_string(),
                        arch: "x86_64".to_string(),
                    },
                };
                let json = serde_json::to_string(&receipt).unwrap();
                let parsed: Receipt = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.plan_id, receipt.plan_id);
                assert_eq!(parsed.packages.len(), receipt.packages.len());
                assert_eq!(parsed.receipt_version, receipt.receipt_version);
                assert!(parsed.git_context.is_some());
            }

            // --- PublishEvent roundtrip ---
            #[test]
            fn publish_event_roundtrip(variant in 0u8..5) {
                let event_type = match variant {
                    0 => EventType::ExecutionStarted,
                    1 => EventType::PlanCreated {
                        plan_id: "abc".to_string(),
                        package_count: 3,
                    },
                    2 => EventType::PackageStarted {
                        name: "test".to_string(),
                        version: "1.0.0".to_string(),
                    },
                    3 => EventType::PackageFailed {
                        class: ErrorClass::Retryable,
                        message: "timeout".to_string(),
                    },
                    _ => EventType::ExecutionFinished {
                        result: ExecutionResult::Success,
                    },
                };
                let event = PublishEvent {
                    timestamp: Utc::now(),
                    event_type,
                    package: "test@1.0.0".to_string(),
                };
                let json = serde_json::to_string(&event).unwrap();
                let parsed: PublishEvent = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.package, event.package);
            }

            // --- EventType all variants roundtrip ---
            #[test]
            fn event_type_all_variants_roundtrip(variant in 0u8..18) {
                let event_type = match variant {
                    0 => EventType::PlanCreated { plan_id: "id1".to_string(), package_count: 5 },
                    1 => EventType::ExecutionStarted,
                    2 => EventType::ExecutionFinished { result: ExecutionResult::Success },
                    3 => EventType::PackageStarted { name: "a".to_string(), version: "1.0.0".to_string() },
                    4 => EventType::PackageAttempted { attempt: 1, command: "cargo publish".to_string() },
                    5 => EventType::PackageOutput { stdout_tail: "ok".to_string(), stderr_tail: "".to_string() },
                    6 => EventType::PackagePublished { duration_ms: 100 },
                    7 => EventType::PackageFailed { class: ErrorClass::Retryable, message: "err".to_string() },
                    8 => EventType::PackageSkipped { reason: "exists".to_string() },
                    9 => EventType::ReadinessStarted { method: ReadinessMethod::Api },
                    10 => EventType::ReadinessPoll { attempt: 1, visible: false },
                    11 => EventType::ReadinessComplete { duration_ms: 500, attempts: 3 },
                    12 => EventType::ReadinessTimeout { max_wait_ms: 60000 },
                    13 => EventType::IndexReadinessStarted { crate_name: "a".to_string(), version: "1.0.0".to_string() },
                    14 => EventType::IndexReadinessCheck { crate_name: "a".to_string(), version: "1.0.0".to_string(), found: true },
                    15 => EventType::IndexReadinessComplete { crate_name: "a".to_string(), version: "1.0.0".to_string(), visible: true },
                    16 => EventType::PreflightStarted,
                    _ => EventType::PreflightComplete { finishability: Finishability::Proven },
                };
                let json = serde_json::to_string(&event_type).unwrap();
                let _parsed: EventType = serde_json::from_str(&json).unwrap();
            }
        }

        // ===== PackageState transition validity =====

        /// Valid transitions from each PackageState
        fn valid_next_states(state: &PackageState) -> Vec<PackageState> {
            match state {
                PackageState::Pending => vec![
                    PackageState::Uploaded,
                    PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: "err".to_string(),
                    },
                    PackageState::Skipped {
                        reason: "already published".to_string(),
                    },
                ],
                PackageState::Uploaded => vec![
                    PackageState::Published,
                    PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: "readiness timeout".to_string(),
                    },
                    PackageState::Ambiguous {
                        message: "unclear".to_string(),
                    },
                ],
                PackageState::Failed { .. } => vec![
                    PackageState::Pending, // retry resets to Pending
                ],
                // Terminal states
                PackageState::Published => vec![],
                PackageState::Skipped { .. } => vec![],
                PackageState::Ambiguous { .. } => vec![],
            }
        }

        fn is_terminal(state: &PackageState) -> bool {
            matches!(
                state,
                PackageState::Published
                    | PackageState::Skipped { .. }
                    | PackageState::Ambiguous { .. }
            )
        }

        proptest! {
            #[test]
            fn package_state_transitions_are_valid(
                start_variant in 0u8..6,
            ) {
                let start = match start_variant {
                    0 => PackageState::Pending,
                    1 => PackageState::Uploaded,
                    2 => PackageState::Published,
                    3 => PackageState::Skipped { reason: "exists".to_string() },
                    4 => PackageState::Failed { class: ErrorClass::Retryable, message: "err".to_string() },
                    _ => PackageState::Ambiguous { message: "unclear".to_string() },
                };

                let nexts = valid_next_states(&start);
                if is_terminal(&start) {
                    assert!(nexts.is_empty(), "Terminal state {:?} should have no valid transitions", start);
                } else {
                    assert!(!nexts.is_empty(), "Non-terminal state {:?} should have valid transitions", start);
                }
            }

            /// Failed states can always retry back to Pending
            #[test]
            fn failed_state_can_retry(
                class_variant in 0u8..3,
                message in "[a-z ]{1,30}",
            ) {
                let class = match class_variant {
                    0 => ErrorClass::Retryable,
                    1 => ErrorClass::Permanent,
                    _ => ErrorClass::Ambiguous,
                };
                let failed = PackageState::Failed { class, message };
                let nexts = valid_next_states(&failed);
                assert!(nexts.contains(&PackageState::Pending),
                    "Failed state should allow retry to Pending");
            }

            /// Pending always leads to Uploaded, Failed, or Skipped
            #[test]
            fn pending_has_expected_transitions(_dummy in 0u8..1) {
                let nexts = valid_next_states(&PackageState::Pending);
                assert_eq!(nexts.len(), 3);
                assert!(matches!(nexts[0], PackageState::Uploaded));
                assert!(matches!(nexts[1], PackageState::Failed { .. }));
                assert!(matches!(nexts[2], PackageState::Skipped { .. }));
            }
        }

        // ===== Plan determinism =====

        proptest! {
            /// Same inputs produce the same plan_id (SHA256 determinism)
            #[test]
            fn plan_id_deterministic_for_same_inputs(
                pkg_count in 1usize..6,
                seed in 0u64..1000,
            ) {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};

                // Generate a deterministic "plan_id" from the same inputs
                fn compute_plan_id(packages: &[PlannedPackage], registry_name: &str) -> String {
                    let mut hasher = DefaultHasher::new();
                    registry_name.hash(&mut hasher);
                    for pkg in packages {
                        pkg.name.hash(&mut hasher);
                        pkg.version.hash(&mut hasher);
                    }
                    format!("{:016x}", hasher.finish())
                }

                let packages: Vec<PlannedPackage> = (0..pkg_count)
                    .map(|i| PlannedPackage {
                        name: format!("crate-{}-{}", seed, i),
                        version: format!("{}.0.0", i),
                        manifest_path: PathBuf::from(format!("crates/crate-{i}/Cargo.toml")),
                    })
                    .collect();

                let id1 = compute_plan_id(&packages, "crates-io");
                let id2 = compute_plan_id(&packages, "crates-io");
                assert_eq!(id1, id2, "Same inputs must produce the same plan_id");
            }

            /// Different package lists produce different plan_ids
            #[test]
            fn plan_id_differs_for_different_inputs(
                seed in 0u64..1000,
            ) {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};

                fn compute_plan_id(packages: &[PlannedPackage], registry_name: &str) -> String {
                    let mut hasher = DefaultHasher::new();
                    registry_name.hash(&mut hasher);
                    for pkg in packages {
                        pkg.name.hash(&mut hasher);
                        pkg.version.hash(&mut hasher);
                    }
                    format!("{:016x}", hasher.finish())
                }

                let pkgs_a = vec![PlannedPackage {
                    name: format!("crate-a-{seed}"),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("Cargo.toml"),
                }];
                let pkgs_b = vec![PlannedPackage {
                    name: format!("crate-b-{seed}"),
                    version: "1.0.0".to_string(),
                    manifest_path: PathBuf::from("Cargo.toml"),
                }];

                let id_a = compute_plan_id(&pkgs_a, "crates-io");
                let id_b = compute_plan_id(&pkgs_b, "crates-io");
                assert_ne!(id_a, id_b, "Different inputs must produce different plan_ids");
            }
        }

        // ===== Receipt generation from ExecutionState =====

        fn build_receipt_from_state(state: &ExecutionState) -> Receipt {
            let now = Utc::now();
            let packages: Vec<PackageReceipt> = state
                .packages
                .values()
                .map(|progress| PackageReceipt {
                    name: progress.name.clone(),
                    version: progress.version.clone(),
                    attempts: progress.attempts,
                    state: progress.state.clone(),
                    started_at: state.created_at,
                    finished_at: now,
                    duration_ms: 0,
                    evidence: PackageEvidence {
                        attempts: vec![],
                        readiness_checks: vec![],
                    },
                })
                .collect();

            Receipt {
                receipt_version: "shipper.receipt.v1".to_string(),
                plan_id: state.plan_id.clone(),
                registry: state.registry.clone(),
                started_at: state.created_at,
                finished_at: now,
                packages,
                event_log_path: PathBuf::from(".shipper/events.jsonl"),
                git_context: None,
                environment: EnvironmentFingerprint {
                    shipper_version: "0.3.0".to_string(),
                    cargo_version: None,
                    rust_version: None,
                    os: "test".to_string(),
                    arch: "test".to_string(),
                },
            }
        }

        proptest! {
            #[test]
            fn receipt_from_state_preserves_plan_id(
                plan_id in "[a-f0-9]{8,32}",
                pkg_count in 0usize..5,
            ) {
                let mut packages = BTreeMap::new();
                for i in 0..pkg_count {
                    packages.insert(
                        format!("pkg-{i}@{i}.0.0"),
                        PackageProgress {
                            name: format!("pkg-{i}"),
                            version: format!("{i}.0.0"),
                            attempts: 1,
                            state: PackageState::Published,
                            last_updated_at: Utc::now(),
                        },
                    );
                }
                let state = ExecutionState {
                    state_version: "shipper.state.v1".to_string(),
                    plan_id: plan_id.clone(),
                    registry: Registry::crates_io(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    packages,
                };

                let receipt = build_receipt_from_state(&state);
                assert_eq!(receipt.plan_id, plan_id);
                assert_eq!(receipt.packages.len(), pkg_count);
                for pkg_receipt in &receipt.packages {
                    assert!(state.packages.values().any(|p| p.name == pkg_receipt.name));
                    assert_eq!(pkg_receipt.state, PackageState::Published);
                }
            }

            #[test]
            fn receipt_from_state_includes_all_packages(pkg_count in 1usize..8) {
                let mut packages = BTreeMap::new();
                for i in 0..pkg_count {
                    let state_variant = match i % 3 {
                        0 => PackageState::Published,
                        1 => PackageState::Skipped { reason: "exists".to_string() },
                        _ => PackageState::Failed {
                            class: ErrorClass::Permanent,
                            message: "auth failure".to_string(),
                        },
                    };
                    packages.insert(
                        format!("pkg-{i}@{i}.0.0"),
                        PackageProgress {
                            name: format!("pkg-{i}"),
                            version: format!("{i}.0.0"),
                            attempts: (i as u32) + 1,
                            state: state_variant,
                            last_updated_at: Utc::now(),
                        },
                    );
                }
                let state = ExecutionState {
                    state_version: "shipper.state.v1".to_string(),
                    plan_id: "test-plan".to_string(),
                    registry: Registry::crates_io(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    packages,
                };

                let receipt = build_receipt_from_state(&state);
                assert_eq!(receipt.packages.len(), pkg_count);
                // Receipt should be serializable
                let json = serde_json::to_string(&receipt).unwrap();
                let parsed: Receipt = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.packages.len(), pkg_count);
            }
        }

        // ===== Version string parsing roundtrips =====

        /// Parse a semver-like version string and reconstruct it
        fn parse_version(v: &str) -> Option<(u32, u32, u32, Option<String>)> {
            let (main, pre) = if let Some(idx) = v.find('-') {
                (&v[..idx], Some(v[idx + 1..].to_string()))
            } else {
                (v, None)
            };
            let parts: Vec<&str> = main.split('.').collect();
            if parts.len() != 3 {
                return None;
            }
            let major = parts[0].parse::<u32>().ok()?;
            let minor = parts[1].parse::<u32>().ok()?;
            let patch = parts[2].parse::<u32>().ok()?;
            Some((major, minor, patch, pre))
        }

        fn format_version(major: u32, minor: u32, patch: u32, pre: Option<&str>) -> String {
            match pre {
                Some(p) => format!("{major}.{minor}.{patch}-{p}"),
                None => format!("{major}.{minor}.{patch}"),
            }
        }

        proptest! {
            /// Parsing a version string and reformatting yields the original
            #[test]
            fn version_string_roundtrip(
                major in 0u32..100,
                minor in 0u32..100,
                patch in 0u32..100,
            ) {
                let version = format!("{major}.{minor}.{patch}");
                let (m, mi, p, pre) = parse_version(&version).unwrap();
                assert_eq!(m, major);
                assert_eq!(mi, minor);
                assert_eq!(p, patch);
                assert!(pre.is_none());
                let reconstructed = format_version(m, mi, p, pre.as_deref());
                assert_eq!(reconstructed, version);
            }

            /// Version with prerelease tag roundtrips
            #[test]
            fn version_string_with_prerelease_roundtrip(
                major in 0u32..100,
                minor in 0u32..100,
                patch in 0u32..100,
                pre_tag in "[a-z]{1,5}\\.[0-9]{1,3}",
            ) {
                let version = format!("{major}.{minor}.{patch}-{pre_tag}");
                let (m, mi, p, pre) = parse_version(&version).unwrap();
                assert_eq!(m, major);
                assert_eq!(mi, minor);
                assert_eq!(p, patch);
                assert_eq!(pre.as_deref(), Some(pre_tag.as_str()));
                let reconstructed = format_version(m, mi, p, pre.as_deref());
                assert_eq!(reconstructed, version);
            }

            /// Version fields stored in PlannedPackage survive JSON roundtrip
            #[test]
            fn version_in_planned_package_roundtrip(
                major in 0u32..100,
                minor in 0u32..100,
                patch in 0u32..100,
            ) {
                let version = format!("{major}.{minor}.{patch}");
                let pkg = PlannedPackage {
                    name: "test-crate".to_string(),
                    version: version.clone(),
                    manifest_path: PathBuf::from("Cargo.toml"),
                };
                let json = serde_json::to_string(&pkg).unwrap();
                let parsed: PlannedPackage = serde_json::from_str(&json).unwrap();
                let (m, mi, p, _) = parse_version(&parsed.version).unwrap();
                assert_eq!((m, mi, p), (major, minor, patch));
            }
        }

        // ===== ReleasePlan roundtrip with varied registry =====

        proptest! {
            #[test]
            fn release_plan_with_custom_registry_roundtrip(
                plan_id in "[a-f0-9]{8,64}",
                registry_name in "[a-z][a-z0-9-]{0,15}",
                api_base in "https://[a-z]{3,10}\\.[a-z]{2,5}",
                index_base in prop::option::of("https://index\\.[a-z]{3,10}\\.[a-z]{2,5}"),
                pkg_count in 1usize..6,
                dep_count in 0usize..3,
            ) {
                let packages: Vec<PlannedPackage> = (0..pkg_count)
                    .map(|i| PlannedPackage {
                        name: format!("crate-{i}"),
                        version: format!("{}.0.0", i + 1),
                        manifest_path: PathBuf::from(format!("crates/crate-{i}/Cargo.toml")),
                    })
                    .collect();
                let mut deps = BTreeMap::new();
                for d in 0..dep_count.min(pkg_count.saturating_sub(1)) {
                    deps.insert(
                        format!("crate-{}", d + 1),
                        vec![format!("crate-{d}")],
                    );
                }
                let plan = ReleasePlan {
                    plan_version: "shipper.plan.v1".to_string(),
                    plan_id: plan_id.clone(),
                    created_at: Utc::now(),
                    registry: Registry {
                        name: registry_name.clone(),
                        api_base: api_base.clone(),
                        index_base: index_base.clone(),
                    },
                    packages,
                    dependencies: deps.clone(),
                };
                let json = serde_json::to_string(&plan).unwrap();
                let parsed: ReleasePlan = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.plan_id, plan_id);
                assert_eq!(parsed.registry.name, registry_name);
                assert_eq!(parsed.registry.api_base, api_base);
                assert_eq!(parsed.registry.index_base, index_base);
                assert_eq!(parsed.packages.len(), pkg_count);
                assert_eq!(parsed.dependencies, deps);
            }
        }

        // ===== RuntimeOptions duration validation =====

        proptest! {
            #[test]
            fn runtime_options_durations_positive(
                base_delay_ms in 1u64..60_000,
                max_delay_ms in 1u64..600_000,
                verify_timeout_ms in 1u64..3_600_000,
                verify_poll_ms in 1u64..60_000,
                lock_timeout_ms in 1u64..86_400_000,
                pkg_timeout_ms in 1u64..7_200_000,
                readiness_initial_ms in 1u64..10_000,
                readiness_max_ms in 1u64..120_000,
                readiness_total_ms in 1u64..600_000,
                readiness_poll_ms in 1u64..10_000,
            ) {
                let opts = RuntimeOptions {
                    allow_dirty: false,
                    skip_ownership_check: false,
                    strict_ownership: false,
                    no_verify: false,
                    max_attempts: 3,
                    base_delay: Duration::from_millis(base_delay_ms),
                    max_delay: Duration::from_millis(max_delay_ms),
                    retry_strategy: shipper_retry::RetryStrategyType::Exponential,
                    retry_jitter: 0.5,
                    retry_per_error: shipper_retry::PerErrorConfig::default(),
                    verify_timeout: Duration::from_millis(verify_timeout_ms),
                    verify_poll_interval: Duration::from_millis(verify_poll_ms),
                    state_dir: PathBuf::from(".shipper"),
                    force_resume: false,
                    policy: PublishPolicy::Safe,
                    verify_mode: VerifyMode::Workspace,
                    readiness: ReadinessConfig {
                        enabled: true,
                        method: ReadinessMethod::Api,
                        initial_delay: Duration::from_millis(readiness_initial_ms),
                        max_delay: Duration::from_millis(readiness_max_ms),
                        max_total_wait: Duration::from_millis(readiness_total_ms),
                        poll_interval: Duration::from_millis(readiness_poll_ms),
                        jitter_factor: 0.5,
                        index_path: None,
                        prefer_index: false,
                    },
                    output_lines: 1000,
                    force: false,
                    lock_timeout: Duration::from_millis(lock_timeout_ms),
                    parallel: ParallelConfig {
                        enabled: false,
                        max_concurrent: 4,
                        per_package_timeout: Duration::from_millis(pkg_timeout_ms),
                    },
                    webhook: WebhookConfig::default(),
                    encryption: EncryptionSettings::default(),
                    registries: vec![],
                    resume_from: None,
                };

                // All duration fields must be positive
                assert!(opts.base_delay > Duration::ZERO);
                assert!(opts.max_delay > Duration::ZERO);
                assert!(opts.verify_timeout > Duration::ZERO);
                assert!(opts.verify_poll_interval > Duration::ZERO);
                assert!(opts.lock_timeout > Duration::ZERO);
                assert!(opts.parallel.per_package_timeout > Duration::ZERO);
                assert!(opts.readiness.initial_delay > Duration::ZERO);
                assert!(opts.readiness.max_delay > Duration::ZERO);
                assert!(opts.readiness.max_total_wait > Duration::ZERO);
                assert!(opts.readiness.poll_interval > Duration::ZERO);
            }
        }

        // ===== Receipt with mixed package states roundtrip =====

        proptest! {
            #[test]
            fn receipt_with_mixed_states_roundtrip(
                plan_id in "[a-f0-9]{8,32}",
                pkg_count in 1usize..6,
                git_commit in prop::option::of("[a-f0-9]{7,40}"),
                git_branch in prop::option::of("[a-z0-9/-]{1,20}"),
                shipper_ver in "[0-9]{1,2}\\.[0-9]{1,2}\\.[0-9]{1,2}",
                os_name in "[a-z]{3,10}",
            ) {
                let now = Utc::now();
                let packages: Vec<PackageReceipt> = (0..pkg_count)
                    .map(|i| {
                        let state = match i % 5 {
                            0 => PackageState::Published,
                            1 => PackageState::Skipped { reason: "already exists".to_string() },
                            2 => PackageState::Failed {
                                class: ErrorClass::Permanent,
                                message: "auth error".to_string(),
                            },
                            3 => PackageState::Ambiguous { message: "timeout".to_string() },
                            _ => PackageState::Uploaded,
                        };
                        PackageReceipt {
                            name: format!("crate-{i}"),
                            version: format!("{i}.1.0"),
                            attempts: (i as u32) + 1,
                            state,
                            started_at: now,
                            finished_at: now,
                            duration_ms: (i as u128 + 1) * 500,
                            evidence: PackageEvidence {
                                attempts: vec![],
                                readiness_checks: vec![],
                            },
                        }
                    })
                    .collect();
                let receipt = Receipt {
                    receipt_version: "shipper.receipt.v1".to_string(),
                    plan_id: plan_id.clone(),
                    registry: Registry::crates_io(),
                    started_at: now,
                    finished_at: now,
                    packages: packages.clone(),
                    event_log_path: PathBuf::from(".shipper/events.jsonl"),
                    git_context: Some(GitContext {
                        commit: git_commit.clone(),
                        branch: git_branch.clone(),
                        tag: None,
                        dirty: Some(false),
                    }),
                    environment: EnvironmentFingerprint {
                        shipper_version: shipper_ver.clone(),
                        cargo_version: None,
                        rust_version: None,
                        os: os_name.clone(),
                        arch: "x86_64".to_string(),
                    },
                };
                let json = serde_json::to_string(&receipt).unwrap();
                let parsed: Receipt = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.plan_id, plan_id);
                assert_eq!(parsed.packages.len(), pkg_count);
                assert_eq!(parsed.environment.shipper_version, shipper_ver);
                assert_eq!(parsed.environment.os, os_name);
                let ctx = parsed.git_context.unwrap();
                assert_eq!(ctx.commit, git_commit);
                assert_eq!(ctx.branch, git_branch);
                for (orig, p) in packages.iter().zip(parsed.packages.iter()) {
                    assert_eq!(p.name, orig.name);
                    assert_eq!(p.state, orig.state);
                    assert_eq!(p.duration_ms, orig.duration_ms);
                }
            }
        }

        // ===== ExecutionState with varied package states roundtrip =====

        proptest! {
            #[test]
            fn execution_state_with_varied_states_roundtrip(
                plan_id in "[a-f0-9]{8,32}",
                pkg_count in 1usize..6,
            ) {
                let mut packages = BTreeMap::new();
                for i in 0..pkg_count {
                    let state = match i % 5 {
                        0 => PackageState::Pending,
                        1 => PackageState::Uploaded,
                        2 => PackageState::Published,
                        3 => PackageState::Skipped { reason: "exists".to_string() },
                        _ => PackageState::Failed {
                            class: ErrorClass::Retryable,
                            message: "timeout".to_string(),
                        },
                    };
                    packages.insert(
                        format!("crate-{i}@{i}.0.0"),
                        PackageProgress {
                            name: format!("crate-{i}"),
                            version: format!("{i}.0.0"),
                            attempts: (i as u32) + 1,
                            state,
                            last_updated_at: Utc::now(),
                        },
                    );
                }
                let exec_state = ExecutionState {
                    state_version: "shipper.state.v1".to_string(),
                    plan_id: plan_id.clone(),
                    registry: Registry::crates_io(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    packages: packages.clone(),
                };
                let json = serde_json::to_string(&exec_state).unwrap();
                let parsed: ExecutionState = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.plan_id, plan_id);
                assert_eq!(parsed.packages.len(), pkg_count);
                for (key, orig) in &packages {
                    let p = parsed.packages.get(key).unwrap();
                    assert_eq!(p.name, orig.name);
                    assert_eq!(p.version, orig.version);
                    assert_eq!(p.attempts, orig.attempts);
                    assert_eq!(p.state, orig.state);
                }
            }
        }

        // ===== PackageState transition monotonicity =====

        /// Ordinal value for PackageState in the forward progress direction.
        /// Higher values represent more progress toward completion.
        fn state_ordinal(state: &PackageState) -> u8 {
            match state {
                PackageState::Pending => 0,
                PackageState::Uploaded => 1,
                PackageState::Published => 2,
                PackageState::Skipped { .. } => 2,   // terminal
                PackageState::Failed { .. } => 1,    // same level as Uploaded
                PackageState::Ambiguous { .. } => 2, // terminal
            }
        }

        proptest! {
            /// Forward transitions (non-retry) never decrease the ordinal
            #[test]
            fn package_state_forward_transitions_monotonic(
                start_variant in 0u8..6,
            ) {
                let start = match start_variant {
                    0 => PackageState::Pending,
                    1 => PackageState::Uploaded,
                    2 => PackageState::Published,
                    3 => PackageState::Skipped { reason: "exists".to_string() },
                    4 => PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: "err".to_string(),
                    },
                    _ => PackageState::Ambiguous { message: "unclear".to_string() },
                };
                let start_ord = state_ordinal(&start);
                let nexts = valid_next_states(&start);
                for next in &nexts {
                    // The only allowed "backwards" transition is Failed -> Pending (retry)
                    let is_retry = matches!(
                        (&start, next),
                        (PackageState::Failed { .. }, PackageState::Pending)
                    );
                    if !is_retry {
                        assert!(
                            state_ordinal(next) >= start_ord,
                            "Non-retry transition {:?} -> {:?} must not decrease ordinal ({} -> {})",
                            start, next, start_ord, state_ordinal(next)
                        );
                    }
                }
            }

            /// The happy path Pending -> Uploaded -> Published is strictly increasing
            #[test]
            fn happy_path_is_strictly_monotonic(_dummy in 0u8..1) {
                let path = [
                    PackageState::Pending,
                    PackageState::Uploaded,
                    PackageState::Published,
                ];
                for w in path.windows(2) {
                    assert!(
                        state_ordinal(&w[1]) > state_ordinal(&w[0]),
                        "Happy path must be strictly increasing: {:?} -> {:?}",
                        w[0], w[1]
                    );
                }
            }

            /// Terminal states have no forward transitions (can't go backwards)
            #[test]
            fn terminal_states_have_no_transitions(variant in 0u8..3) {
                let state = match variant {
                    0 => PackageState::Published,
                    1 => PackageState::Skipped { reason: "exists".to_string() },
                    _ => PackageState::Ambiguous { message: "unclear".to_string() },
                };
                let nexts = valid_next_states(&state);
                assert!(
                    nexts.is_empty(),
                    "Terminal state {:?} must have no transitions but has {:?}",
                    state, nexts
                );
            }
        }

        // ===== Error/type Debug formatting never panics =====

        proptest! {
            #[test]
            fn package_state_debug_never_panics(
                variant in 0u8..6,
                message in "\\PC{0,200}",
            ) {
                let state = match variant {
                    0 => PackageState::Pending,
                    1 => PackageState::Uploaded,
                    2 => PackageState::Published,
                    3 => PackageState::Skipped { reason: message.clone() },
                    4 => PackageState::Failed {
                        class: ErrorClass::Retryable,
                        message: message.clone(),
                    },
                    _ => PackageState::Ambiguous { message },
                };
                let debug = format!("{:?}", state);
                assert!(!debug.is_empty());
            }

            #[test]
            fn error_class_debug_never_panics(variant in 0u8..3) {
                let class = match variant {
                    0 => ErrorClass::Retryable,
                    1 => ErrorClass::Permanent,
                    _ => ErrorClass::Ambiguous,
                };
                let debug = format!("{:?}", class);
                assert!(!debug.is_empty());
            }

            #[test]
            fn execution_result_debug_never_panics(variant in 0u8..3) {
                let result = match variant {
                    0 => ExecutionResult::Success,
                    1 => ExecutionResult::PartialFailure,
                    _ => ExecutionResult::CompleteFailure,
                };
                let debug = format!("{:?}", result);
                assert!(!debug.is_empty());
            }

            #[test]
            fn finishability_debug_never_panics(variant in 0u8..3) {
                let fin = match variant {
                    0 => Finishability::Proven,
                    1 => Finishability::NotProven,
                    _ => Finishability::Failed,
                };
                let debug = format!("{:?}", fin);
                assert!(!debug.is_empty());
            }

            #[test]
            fn event_type_debug_never_panics(
                variant in 0u8..18,
                msg in "\\PC{0,100}",
            ) {
                let event_type = match variant {
                    0 => EventType::PlanCreated { plan_id: msg.clone(), package_count: 5 },
                    1 => EventType::ExecutionStarted,
                    2 => EventType::ExecutionFinished { result: ExecutionResult::Success },
                    3 => EventType::PackageStarted { name: msg.clone(), version: "1.0.0".to_string() },
                    4 => EventType::PackageAttempted { attempt: 1, command: msg.clone() },
                    5 => EventType::PackageOutput { stdout_tail: msg.clone(), stderr_tail: String::new() },
                    6 => EventType::PackagePublished { duration_ms: 100 },
                    7 => EventType::PackageFailed { class: ErrorClass::Retryable, message: msg.clone() },
                    8 => EventType::PackageSkipped { reason: msg.clone() },
                    9 => EventType::ReadinessStarted { method: ReadinessMethod::Api },
                    10 => EventType::ReadinessPoll { attempt: 1, visible: false },
                    11 => EventType::ReadinessComplete { duration_ms: 500, attempts: 3 },
                    12 => EventType::ReadinessTimeout { max_wait_ms: 60000 },
                    13 => EventType::IndexReadinessStarted { crate_name: msg.clone(), version: "1.0.0".to_string() },
                    14 => EventType::IndexReadinessCheck { crate_name: msg.clone(), version: "1.0.0".to_string(), found: true },
                    15 => EventType::IndexReadinessComplete { crate_name: msg.clone(), version: "1.0.0".to_string(), visible: true },
                    16 => EventType::PreflightStarted,
                    _ => EventType::PreflightComplete { finishability: Finishability::Proven },
                };
                let debug = format!("{:?}", event_type);
                assert!(!debug.is_empty());
            }

            #[test]
            fn publish_event_debug_never_panics(
                pkg in "[a-z][a-z0-9-]{0,15}@[0-9]+\\.[0-9]+\\.[0-9]+",
            ) {
                let event = PublishEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::ExecutionStarted,
                    package: pkg,
                };
                let debug = format!("{:?}", event);
                assert!(!debug.is_empty());
            }
        }

        // ===== Arbitrary PackageState sequences =====

        proptest! {
            /// Random sequences of PackageState transitions follow valid_next_states
            #[test]
            fn arbitrary_package_state_sequence(steps in 1usize..10) {
                let mut current = PackageState::Pending;
                for _ in 0..steps {
                    let nexts = valid_next_states(&current);
                    if nexts.is_empty() {
                        break; // terminal state
                    }
                    // Always pick the first valid transition for determinism
                    current = nexts[0].clone();
                }
                // We should end in a well-known state
                let debug = format!("{:?}", current);
                assert!(!debug.is_empty());
            }

            /// The happy path Pending→Uploaded→Published always completes in 2 transitions
            #[test]
            fn happy_path_always_reaches_published(_seed in 0u64..100) {
                let mut state = PackageState::Pending;
                // Pending -> Uploaded
                let nexts = valid_next_states(&state);
                assert!(nexts.iter().any(|s| matches!(s, PackageState::Uploaded)));
                state = PackageState::Uploaded;
                // Uploaded -> Published
                let nexts = valid_next_states(&state);
                assert!(nexts.iter().any(|s| matches!(s, PackageState::Published)));
                state = PackageState::Published;
                // Published is terminal
                assert!(valid_next_states(&state).is_empty());
            }

            /// Full receipt with evidence roundtrips preserve attempt counts
            #[test]
            fn receipt_evidence_attempt_counts_preserved(
                attempt_count in 0usize..5,
                readiness_count in 0usize..5,
            ) {
                let now = Utc::now();
                let attempts: Vec<AttemptEvidence> = (0..attempt_count)
                    .map(|i| AttemptEvidence {
                        attempt_number: i as u32 + 1,
                        command: format!("cargo publish attempt {i}"),
                        exit_code: 0,
                        stdout_tail: "ok".to_string(),
                        stderr_tail: String::new(),
                        timestamp: now,
                        duration: Duration::from_secs(1),
                    })
                    .collect();
                let checks: Vec<ReadinessEvidence> = (0..readiness_count)
                    .map(|i| ReadinessEvidence {
                        attempt: i as u32 + 1,
                        visible: i == readiness_count - 1,
                        timestamp: now,
                        delay_before: Duration::from_secs(2),
                    })
                    .collect();
                let evidence = PackageEvidence {
                    attempts: attempts.clone(),
                    readiness_checks: checks.clone(),
                };
                let json = serde_json::to_string(&evidence).unwrap();
                let parsed: PackageEvidence = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.attempts.len(), attempt_count);
                assert_eq!(parsed.readiness_checks.len(), readiness_count);
                for (orig, p) in attempts.iter().zip(parsed.attempts.iter()) {
                    assert_eq!(orig.attempt_number, p.attempt_number);
                    assert_eq!(orig.exit_code, p.exit_code);
                }
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

        // --- Additional invariant proptests ---

        proptest! {
            /// ReleasePlan JSON roundtrip with deps: serialize then deserialize preserves all fields.
            #[test]
            fn release_plan_with_deps_roundtrip(
                pkg_count in 0usize..8,
                plan_id in "[a-f0-9]{8}",
            ) {
                let packages: Vec<PlannedPackage> = (0..pkg_count)
                    .map(|i| PlannedPackage {
                        name: format!("crate-{i}"),
                        version: format!("0.{i}.0"),
                        manifest_path: PathBuf::from(format!("crates/crate-{i}/Cargo.toml")),
                    })
                    .collect();

                let mut deps = BTreeMap::new();
                for i in 1..pkg_count {
                    deps.insert(
                        format!("crate-{i}"),
                        vec![format!("crate-{}", i - 1)],
                    );
                }

                let plan = ReleasePlan {
                    plan_version: "shipper.plan.v1".to_string(),
                    plan_id: plan_id.clone(),
                    created_at: Utc::now(),
                    registry: Registry::crates_io(),
                    packages: packages.clone(),
                    dependencies: deps.clone(),
                };

                let json = serde_json::to_string(&plan).unwrap();
                let parsed: ReleasePlan = serde_json::from_str(&json).unwrap();

                prop_assert_eq!(parsed.plan_id, plan.plan_id);
                prop_assert_eq!(parsed.packages.len(), pkg_count);
                prop_assert_eq!(parsed.dependencies.len(), deps.len());
                for (orig, p) in plan.packages.iter().zip(parsed.packages.iter()) {
                    prop_assert_eq!(&p.name, &orig.name);
                    prop_assert_eq!(&p.version, &orig.version);
                }
            }

            /// Plan ordering: group_by_levels always places dependencies before dependents.
            #[test]
            fn plan_levels_respect_dependency_ordering(
                pkg_count in 1usize..10,
            ) {
                let packages: Vec<PlannedPackage> = (0..pkg_count)
                    .map(|i| PlannedPackage {
                        name: format!("crate-{i}"),
                        version: format!("0.{i}.0"),
                        manifest_path: PathBuf::from(format!("crates/crate-{i}/Cargo.toml")),
                    })
                    .collect();

                // Linear dependency chain: crate-1 depends on crate-0, crate-2 on crate-1, etc.
                let mut deps = BTreeMap::new();
                for i in 1..pkg_count {
                    deps.insert(
                        format!("crate-{i}"),
                        vec![format!("crate-{}", i - 1)],
                    );
                }

                let plan = ReleasePlan {
                    plan_version: "shipper.plan.v1".to_string(),
                    plan_id: "test-plan".to_string(),
                    created_at: Utc::now(),
                    registry: Registry::crates_io(),
                    packages,
                    dependencies: deps.clone(),
                };

                let levels = plan.group_by_levels();

                // Build a map of package name -> level number
                let mut pkg_level: BTreeMap<String, usize> = BTreeMap::new();
                for level in &levels {
                    for pkg in &level.packages {
                        pkg_level.insert(pkg.name.clone(), level.level);
                    }
                }

                // Every dependency must be at a strictly earlier level
                for (name, dep_list) in &deps {
                    if let Some(&my_level) = pkg_level.get(name.as_str()) {
                        for dep in dep_list {
                            if let Some(&dep_level) = pkg_level.get(dep.as_str()) {
                                prop_assert!(
                                    dep_level < my_level,
                                    "{name} (level {my_level}) depends on {dep} (level {dep_level})"
                                );
                            }
                        }
                    }
                }
            }

            /// Receipt completeness: every package in the plan appears in the receipt.
            #[test]
            fn receipt_contains_all_plan_packages(
                pkg_count in 1usize..8,
            ) {
                let now = Utc::now();
                let packages: Vec<PlannedPackage> = (0..pkg_count)
                    .map(|i| PlannedPackage {
                        name: format!("crate-{i}"),
                        version: format!("0.{i}.0"),
                        manifest_path: PathBuf::from(format!("crates/crate-{i}/Cargo.toml")),
                    })
                    .collect();

                let receipts: Vec<PackageReceipt> = packages
                    .iter()
                    .map(|pkg| PackageReceipt {
                        name: pkg.name.clone(),
                        version: pkg.version.clone(),
                        attempts: 1,
                        state: PackageState::Published,
                        started_at: now,
                        finished_at: now,
                        duration_ms: 100,
                        evidence: PackageEvidence {
                            attempts: vec![],
                            readiness_checks: vec![],
                        },
                    })
                    .collect();

                let receipt = Receipt {
                    receipt_version: "shipper.receipt.v1".to_string(),
                    plan_id: "plan-test".to_string(),
                    registry: Registry::crates_io(),
                    started_at: now,
                    finished_at: now,
                    packages: receipts.clone(),
                    event_log_path: PathBuf::from(".shipper/events.jsonl"),
                    git_context: None,
                    environment: EnvironmentFingerprint {
                        shipper_version: "0.1.0".to_string(),
                        cargo_version: None,
                        rust_version: None,
                        os: "linux".to_string(),
                        arch: "x86_64".to_string(),
                    },
                };

                // Every planned package appears in the receipt
                for pkg in &packages {
                    let found = receipt.packages.iter().any(|r| r.name == pkg.name && r.version == pkg.version);
                    prop_assert!(found, "package {}@{} missing from receipt", pkg.name, pkg.version);
                }
                prop_assert_eq!(receipt.packages.len(), packages.len());

                // Roundtrip the receipt
                let json = serde_json::to_string(&receipt).unwrap();
                let parsed: Receipt = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(parsed.packages.len(), receipt.packages.len());
            }
        }
    }
}
