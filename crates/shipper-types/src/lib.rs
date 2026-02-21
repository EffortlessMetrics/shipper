//! Core domain types for shipper.
//!
//! This crate provides the fundamental types used across the shipper ecosystem
//! for crate publishing, error handling, and configuration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorClass::Retryable => write!(f, "retryable"),
            ErrorClass::Ambiguous => write!(f, "ambiguous"),
            ErrorClass::Permanent => write!(f, "permanent"),
        }
    }
}

/// Registry configuration for publishing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    /// Registry name (e.g., "crates-io", "my-registry")
    pub name: String,
    /// API base URL (e.g., "https://crates.io")
    pub api_base: String,
    /// Optional index base URL for sparse index
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_base: Option<String>,
}

impl Registry {
    /// Create a crates.io registry configuration.
    pub fn crates_io() -> Self {
        Self {
            name: "crates-io".to_string(),
            api_base: "https://crates.io".to_string(),
            index_base: None,
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::crates_io()
    }
}

/// Package reference in a publish plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageRef {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
}

/// Publish policy presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishPolicy {
    /// Verify all packages, strict checks
    #[default]
    Safe,
    /// Verify when needed, balanced approach
    Balanced,
    /// No verification, fastest option
    Fast,
}

/// Verification mode for registry visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// Verify all packages in workspace
    #[default]
    Workspace,
    /// Verify each package individually after publish
    Package,
    /// No verification
    None,
}

/// Readiness check method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessMethod {
    /// Check via API (fast)
    #[default]
    Api,
    /// Check via sparse index (slower, more accurate)
    Index,
    /// Check both API and index (slowest, most reliable)
    Both,
}

/// Authentication type detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    /// Token-based authentication
    Token,
    /// GitHub Trusted Publishing
    TrustedPublishing,
    /// Unknown authentication method
    Unknown,
}

/// Package state in the publish process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PackageState {
    /// Package has been successfully published
    Published,
    /// Package is waiting to be published
    Pending,
    /// Package has been uploaded but not yet verified
    Uploaded,
    /// Package was skipped
    Skipped {
        /// Reason for skipping
        reason: String,
    },
    /// Package failed to publish
    Failed {
        /// Error classification
        class: ErrorClass,
        /// Error message
        message: String,
    },
    /// Package publish status is ambiguous
    Ambiguous {
        /// Message describing the ambiguity
        message: String,
    },
}

/// Finishability assessment result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finishability {
    /// All checks passed, ready to publish
    Proven,
    /// Some checks could not be verified
    NotProven,
    /// Checks failed
    Failed,
}

/// Event types for the event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventType {
    /// Plan was created
    PlanCreated {
        plan_id: String,
        package_count: usize,
    },
    /// Execution started
    ExecutionStarted,
    /// Execution finished
    ExecutionFinished { result: ExecutionResult },
    /// Package publish started
    PackageStarted { name: String, version: String },
    /// Package publish attempted
    PackageAttempted { attempt: u32, command: String },
    /// Package output captured
    PackageOutput { stdout_tail: String, stderr_tail: String },
    /// Package published successfully
    PackagePublished { duration_ms: u64 },
    /// Package failed to publish
    PackageFailed { class: ErrorClass, message: String },
    /// Package was skipped
    PackageSkipped { reason: String },
    /// Readiness check started
    ReadinessStarted { method: ReadinessMethod },
    /// Readiness poll result
    ReadinessPoll { attempt: u32, visible: bool },
    /// Readiness check completed
    ReadinessComplete { duration_ms: u64, attempts: u32 },
    /// Readiness check timed out
    ReadinessTimeout { max_wait_ms: u64 },
    /// Preflight started
    PreflightStarted,
    /// Preflight workspace verify result
    PreflightWorkspaceVerify { passed: bool, output: String },
    /// New crate detected during preflight
    PreflightNewCrateDetected { crate_name: String },
    /// Preflight completed
    PreflightComplete { finishability: Finishability },
}

/// Execution result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionResult {
    /// Execution succeeded
    Success,
    /// Execution failed
    Failed,
    /// Execution was partial
    Partial,
}

/// A publish event for the event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishEvent {
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// The type of event
    #[serde(flatten)]
    pub event_type: EventType,
    /// Package this event relates to (or "all" for workspace-level events)
    pub package: String,
}

/// Environment fingerprint for reproducibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentFingerprint {
    /// Shipper version
    pub shipper_version: String,
    /// Cargo version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cargo_version: Option<String>,
    /// Rust version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_version: Option<String>,
    /// Operating system
    pub os: String,
    /// Architecture
    pub arch: String,
}

/// Git context for audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitContext {
    /// Current commit hash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Current branch name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Current tag
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Whether the working tree is dirty
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_class_display() {
        assert_eq!(ErrorClass::Retryable.to_string(), "retryable");
        assert_eq!(ErrorClass::Ambiguous.to_string(), "ambiguous");
        assert_eq!(ErrorClass::Permanent.to_string(), "permanent");
    }

    #[test]
    fn registry_crates_io() {
        let reg = Registry::crates_io();
        assert_eq!(reg.name, "crates-io");
        assert_eq!(reg.api_base, "https://crates.io");
    }

    #[test]
    fn error_class_serde() {
        let json = serde_json::to_string(&ErrorClass::Ambiguous).unwrap();
        assert_eq!(json, "\"ambiguous\"");

        let parsed: ErrorClass = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ErrorClass::Ambiguous);
    }

    #[test]
    fn publish_policy_serde() {
        let json = serde_json::to_string(&PublishPolicy::Fast).unwrap();
        assert_eq!(json, "\"fast\"");

        let parsed: PublishPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, PublishPolicy::Fast);
    }

    #[test]
    fn package_state_serialization() {
        let state = PackageState::Published;
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"state\":\"published\""));

        let failed = PackageState::Failed {
            class: ErrorClass::Permanent,
            message: "auth error".to_string(),
        };
        let json = serde_json::to_string(&failed).unwrap();
        assert!(json.contains("\"state\":\"failed\""));
        assert!(json.contains("\"class\":\"permanent\""));
    }

    #[test]
    fn event_type_serialization() {
        let event = PublishEvent {
            timestamp: Utc::now(),
            event_type: EventType::PackageStarted {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
            },
            package: "test@1.0.0".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"package_started\""));
        assert!(json.contains("\"name\":\"test\""));

        let parsed: PublishEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.package, "test@1.0.0");
    }

    #[test]
    fn finishability_serde() {
        assert_eq!(
            serde_json::to_string(&Finishability::Proven).unwrap(),
            "\"proven\""
        );
    }

    #[test]
    fn environment_fingerprint_serialization() {
        let fp = EnvironmentFingerprint {
            shipper_version: "0.2.0".to_string(),
            cargo_version: Some("1.75.0".to_string()),
            rust_version: Some("1.75.0".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };

        let json = serde_json::to_string(&fp).unwrap();
        assert!(json.contains("\"shipper_version\":\"0.2.0\""));
        assert!(json.contains("\"os\":\"linux\""));
    }
}