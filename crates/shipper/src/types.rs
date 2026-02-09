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
