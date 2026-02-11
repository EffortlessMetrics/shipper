use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    /// Cargo registry name (for `cargo publish --registry <name>`). For crates.io this is typically `crates-io`.
    pub name: String,
    /// Base URL for registry web API, e.g. `https://crates.io`.
    pub api_base: String,
}

impl Registry {
    pub fn crates_io() -> Self {
        Self {
            name: "crates-io".to_string(),
            api_base: "https://crates.io".to_string(),
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
    #[serde(deserialize_with = "deserialize_duration")]
    pub initial_delay: Duration,
    /// Maximum delay between polls (capped)
    #[serde(deserialize_with = "deserialize_duration")]
    pub max_delay: Duration,
    /// Maximum total time to wait for visibility
    #[serde(deserialize_with = "deserialize_duration")]
    pub max_total_wait: Duration,
    /// Base poll interval
    #[serde(deserialize_with = "deserialize_duration")]
    pub poll_interval: Duration,
    /// Jitter factor (±50% means 0.5)
    pub jitter_factor: f64,
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
    #[serde(deserialize_with = "deserialize_duration")]
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
pub struct Receipt {
    pub receipt_version: String,
    pub plan_id: String,
    pub registry: Registry,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub packages: Vec<PackageReceipt>,
    pub event_log_path: PathBuf,
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

    // Preflight events
    PreflightStarted,
    PreflightWorkspaceVerify {
        passed: bool,
    },
    PreflightNewCrateDetected {
        name: String,
        auth_type: AuthType,
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
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Finishability {
    Proven,
    NotProven,
    Failed,
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
        };
        assert!(!config.enabled);
        assert_eq!(config.method, ReadinessMethod::Both);
        assert_eq!(config.initial_delay, Duration::from_millis(500));
        assert_eq!(config.max_delay, Duration::from_secs(30));
        assert_eq!(config.max_total_wait, Duration::from_secs(600));
        assert_eq!(config.poll_interval, Duration::from_secs(5));
        assert_eq!(config.jitter_factor, 0.25);
    }
}
