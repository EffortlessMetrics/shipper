use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DurationMilliSeconds, serde_as};

/// Deserialize a Duration from either a string (human-readable) or u64 (milliseconds)
pub(crate) fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
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
pub(crate) fn serialize_duration<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(duration.as_millis() as u64)
}

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

#[derive(Debug, Clone)]
pub struct ReleaseSpec {
    pub manifest_path: PathBuf,
    pub registry: Registry,
    pub selected_packages: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishPolicy {
    /// Verify + strict checks (default)
    #[default]
    Safe,
    /// Verify only when needed
    Balanced,
    /// No verify; explicit risk
    Fast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// Default, safest - run workspace dry-run
    #[default]
    Workspace,
    /// Per-crate verify
    Package,
    /// No verify
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessMethod {
    /// Check crates.io HTTP API (default, fast)
    #[default]
    Api,
    /// Check sparse index (slower, more accurate)
    Index,
    /// Check both (slowest, most reliable)
    Both,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessConfig {
    /// Enable readiness checks
    pub enabled: bool,
    /// Method for checking version visibility
    pub method: ReadinessMethod,
    /// Initial delay before first poll
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub initial_delay: Duration,
    /// Maximum delay between polls (capped)
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub max_delay: Duration,
    /// Maximum total time to wait for visibility
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub max_total_wait: Duration,
    /// Base poll interval
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub poll_interval: Duration,
    /// Jitter factor (±50% means 0.5)
    pub jitter_factor: f64,
    /// Custom index path for testing (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_path: Option<PathBuf>,
    /// Use index as primary method when Both is selected
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

/// Configuration for parallel publishing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelConfig {
    /// Enable parallel publishing (default: false for sequential)
    pub enabled: bool,
    /// Maximum number of concurrent publish operations (default: 4)
    pub max_concurrent: usize,
    /// Timeout per package publish operation (default: 30 minutes)
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

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub allow_dirty: bool,
    pub skip_ownership_check: bool,
    pub strict_ownership: bool,
    pub no_verify: bool,
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedPackage {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
}

/// A group of packages that can be published in parallel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishLevel {
    /// The level number (0 = no dependencies, 1 = depends on level 0, etc.)
    pub level: usize,
    /// Packages that can be published in parallel at this level
    pub packages: Vec<PlannedPackage>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackageState {
    Pending,
    Published,
    Skipped { reason: String },
    Failed { class: ErrorClass, message: String },
    Ambiguous { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Retryable,
    Permanent,
    Ambiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageProgress {
    pub name: String,
    pub version: String,
    pub attempts: u32,
    pub state: PackageState,
    pub last_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionState {
    pub state_version: String,
    pub plan_id: String,
    pub registry: Registry,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub packages: BTreeMap<String, PackageProgress>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageEvidence {
    pub attempts: Vec<AttemptEvidence>,
    pub readiness_checks: Vec<ReadinessEvidence>,
}

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

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessEvidence {
    pub attempt: u32,
    pub visible: bool,
    pub timestamp: DateTime<Utc>,
    #[serde_as(as = "DurationMilliSeconds<u64>")]
    pub delay_before: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentFingerprint {
    pub shipper_version: String,
    pub cargo_version: Option<String>,
    pub rust_version: Option<String>,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitContext {
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub dirty: Option<bool>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub package: String, // "name@version"
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionResult {
    Success,
    PartialFailure,
    CompleteFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    Token,
    TrustedPublishing,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Finishability {
    Proven,
    NotProven,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub plan_id: String,
    pub token_detected: bool,
    pub finishability: Finishability,
    pub packages: Vec<PreflightPackage>,
    pub timestamp: DateTime<Utc>,
}

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
