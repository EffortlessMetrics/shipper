use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedPackage {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePlan {
    pub plan_id: String,
    pub created_at: DateTime<Utc>,
    pub registry: Registry,
    /// Packages in publish order (dependencies first).
    pub packages: Vec<PlannedPackage>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_version: String,
    pub plan_id: String,
    pub registry: Registry,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub packages: Vec<PackageReceipt>,
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
}
