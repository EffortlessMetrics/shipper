//! Environment fingerprinting for shipper.
//!
//! This crate provides environment detection and fingerprinting
//! for reproducible publish operations across CI environments.
//!
//! # Example
//!
//! ```
//! use shipper_environment::{CiEnvironment, detect_environment, get_environment_fingerprint};
//!
//! // Detect the current CI environment
//! let env = detect_environment();
//! println!("Running in: {:?}", env);
//!
//! // Get a fingerprint for reproducibility
//! let fingerprint = get_environment_fingerprint();
//! println!("Fingerprint: {}", fingerprint);
//! ```

use std::env;
use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Detected CI environment
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiEnvironment {
    /// GitHub Actions
    GitHubActions,
    /// GitLab CI
    GitLabCI,
    /// CircleCI
    CircleCI,
    /// Travis CI
    TravisCI,
    /// Azure Pipelines
    AzurePipelines,
    /// Jenkins
    Jenkins,
    /// Bitbucket Pipelines
    BitbucketPipelines,
    /// No CI detected (local)
    #[default]
    Local,
}

impl std::fmt::Display for CiEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiEnvironment::GitHubActions => write!(f, "GitHub Actions"),
            CiEnvironment::GitLabCI => write!(f, "GitLab CI"),
            CiEnvironment::CircleCI => write!(f, "CircleCI"),
            CiEnvironment::TravisCI => write!(f, "Travis CI"),
            CiEnvironment::AzurePipelines => write!(f, "Azure Pipelines"),
            CiEnvironment::Jenkins => write!(f, "Jenkins"),
            CiEnvironment::BitbucketPipelines => write!(f, "Bitbucket Pipelines"),
            CiEnvironment::Local => write!(f, "Local"),
        }
    }
}

/// Detect the current CI environment
pub fn detect_environment() -> CiEnvironment {
    // GitHub Actions
    if env::var("GITHUB_ACTIONS").is_ok() {
        return CiEnvironment::GitHubActions;
    }

    // GitLab CI
    if env::var("GITLAB_CI").is_ok() {
        return CiEnvironment::GitLabCI;
    }

    // CircleCI
    if env::var("CIRCLECI").is_ok() {
        return CiEnvironment::CircleCI;
    }

    // Travis CI
    if env::var("TRAVIS").is_ok() {
        return CiEnvironment::TravisCI;
    }

    // Azure Pipelines
    if env::var("TF_BUILD").is_ok() {
        return CiEnvironment::AzurePipelines;
    }

    // Jenkins
    if env::var("JENKINS_URL").is_ok() {
        return CiEnvironment::Jenkins;
    }

    // Bitbucket Pipelines
    if env::var("BITBUCKET_BUILD_NUMBER").is_ok() {
        return CiEnvironment::BitbucketPipelines;
    }

    CiEnvironment::Local
}

/// Check if running in any CI environment
pub fn is_ci() -> bool {
    detect_environment() != CiEnvironment::Local
}

/// Environment information collected for fingerprinting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    /// Detected CI environment
    pub ci_environment: CiEnvironment,
    /// Operating system
    pub os: String,
    /// Architecture
    pub arch: String,
    /// Rust version
    pub rust_version: String,
    /// Cargo version
    pub cargo_version: String,
    /// Collected environment variables (sanitized)
    pub env_vars: BTreeMap<String, String>,
    /// Timestamp of collection
    pub collected_at: DateTime<Utc>,
}

impl EnvironmentInfo {
    /// Collect current environment information
    pub fn collect() -> Result<Self> {
        let ci_environment = detect_environment();
        let os = env::consts::OS.to_string();
        let arch = env::consts::ARCH.to_string();
        
        // Get rust version
        let rust_version = get_rust_version().unwrap_or_else(|_| "unknown".to_string());
        
        // Get cargo version
        let cargo_version = get_cargo_version().unwrap_or_else(|_| "unknown".to_string());
        
        // Collect relevant environment variables (sanitized)
        let env_vars = collect_env_vars();
        
        Ok(Self {
            ci_environment,
            os,
            arch,
            rust_version,
            cargo_version,
            env_vars,
            collected_at: Utc::now(),
        })
    }

    /// Generate a fingerprint string
    pub fn fingerprint(&self) -> String {
        let mut components = Vec::new();
        components.push(format!("ci:{}", self.ci_environment));
        components.push(format!("os:{}", self.os));
        components.push(format!("arch:{}", self.arch));
        components.push(format!("rust:{}", self.rust_version));
        components.push(format!("cargo:{}", self.cargo_version));
        
        // Add relevant env vars
        for (key, value) in &self.env_vars {
            components.push(format!("{}:{}", key, value));
        }
        
        components.join("|")
    }
}

/// Get a quick environment fingerprint
pub fn get_environment_fingerprint() -> String {
    let ci = detect_environment();
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    let rust = get_rust_version().unwrap_or_else(|_| "unknown".to_string());
    
    format!("{}|{}|{}|{}", ci, os, arch, rust)
}

/// Get the Rust version
pub fn get_rust_version() -> Result<String> {
    let output = std::process::Command::new("rustc")
        .args(["--version"])
        .output()?;
    
    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    } else {
        Err(anyhow::anyhow!("failed to get rust version"))
    }
}

/// Get the Cargo version
pub fn get_cargo_version() -> Result<String> {
    let output = std::process::Command::new("cargo")
        .args(["--version"])
        .output()?;
    
    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(version)
    } else {
        Err(anyhow::anyhow!("failed to get cargo version"))
    }
}

/// Collect relevant environment variables for fingerprinting
fn collect_env_vars() -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    
    // CI-related variables that are safe to include
    let ci_vars = [
        "CI",
        "GITHUB_REF",
        "GITHUB_SHA",
        "GITHUB_REPOSITORY",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_NUMBER",
        "GITLAB_CI_PIPELINE_ID",
        "CIRCLE_BUILD_NUM",
        "CIRCLE_BRANCH",
        "TRAVIS_BUILD_NUMBER",
        "TRAVIS_BRANCH",
        "BUILD_BUILDID",
        "BUILD_NUMBER",
        "BITBUCKET_BRANCH",
        "BITBUCKET_COMMIT",
    ];
    
    for var in ci_vars {
        if let Ok(value) = env::var(var) {
            vars.insert(var.to_string(), value);
        }
    }
    
    vars
}

/// Get the current branch name from CI environment
pub fn get_ci_branch() -> Option<String> {
    let env = detect_environment();
    
    match env {
        CiEnvironment::GitHubActions => env::var("GITHUB_REF_NAME").ok(),
        CiEnvironment::GitLabCI => env::var("CI_COMMIT_REF_NAME").ok(),
        CiEnvironment::CircleCI => env::var("CIRCLE_BRANCH").ok(),
        CiEnvironment::TravisCI => env::var("TRAVIS_BRANCH").ok(),
        CiEnvironment::AzurePipelines => env::var("BUILD_SOURCEBRANCHNAME").ok(),
        CiEnvironment::Jenkins => env::var("GIT_BRANCH").ok(),
        CiEnvironment::BitbucketPipelines => env::var("BITBUCKET_BRANCH").ok(),
        CiEnvironment::Local => None,
    }
}

/// Get the current commit SHA from CI environment
pub fn get_ci_commit_sha() -> Option<String> {
    let env = detect_environment();
    
    match env {
        CiEnvironment::GitHubActions => env::var("GITHUB_SHA").ok(),
        CiEnvironment::GitLabCI => env::var("CI_COMMIT_SHA").ok(),
        CiEnvironment::CircleCI => env::var("CIRCLE_SHA1").ok(),
        CiEnvironment::TravisCI => env::var("TRAVIS_COMMIT").ok(),
        CiEnvironment::AzurePipelines => env::var("BUILD_SOURCEVERSION").ok(),
        CiEnvironment::Jenkins => env::var("GIT_COMMIT").ok(),
        CiEnvironment::BitbucketPipelines => env::var("BITBUCKET_COMMIT").ok(),
        CiEnvironment::Local => None,
    }
}

/// Check if running on a pull request
pub fn is_pull_request() -> bool {
    let env = detect_environment();
    
    match env {
        CiEnvironment::GitHubActions => {
            env::var("GITHUB_EVENT_NAME").map(|v| v == "pull_request").unwrap_or(false)
        }
        CiEnvironment::GitLabCI => {
            env::var("CI_MERGE_REQUEST_ID").is_ok()
        }
        CiEnvironment::CircleCI => {
            env::var("CIRCLE_PULL_REQUEST").is_ok()
        }
        CiEnvironment::TravisCI => {
            env::var("TRAVIS_PULL_REQUEST").map(|v| v != "false").unwrap_or(false)
        }
        CiEnvironment::AzurePipelines => {
            env::var("BUILD_REASON").map(|v| v == "PullRequest").unwrap_or(false)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_environment_display() {
        assert_eq!(CiEnvironment::GitHubActions.to_string(), "GitHub Actions");
        assert_eq!(CiEnvironment::GitLabCI.to_string(), "GitLab CI");
        assert_eq!(CiEnvironment::Local.to_string(), "Local");
    }

    #[test]
    fn ci_environment_default() {
        let env = CiEnvironment::default();
        assert_eq!(env, CiEnvironment::Local);
    }

    #[test]
    fn detect_environment_runs() {
        // Just verify it doesn't panic
        let _ = detect_environment();
    }

    #[test]
    fn is_ci_runs() {
        // Just verify it doesn't panic
        let _ = is_ci();
    }

    #[test]
    fn get_environment_fingerprint_runs() {
        let fp = get_environment_fingerprint();
        assert!(!fp.is_empty());
        assert!(fp.contains('|'));
    }

    #[test]
    fn environment_info_fingerprint_format() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::Local,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.70.0".to_string(),
            cargo_version: "1.70.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };

        let fp = info.fingerprint();
        assert!(fp.contains("ci:Local"));
        assert!(fp.contains("os:linux"));
        assert!(fp.contains("arch:x86_64"));
    }

    #[test]
    fn get_rust_version_succeeds() {
        let version = get_rust_version();
        assert!(version.is_ok());
        let v = version.unwrap();
        assert!(v.starts_with("rustc"));
    }

    #[test]
    fn get_cargo_version_succeeds() {
        let version = get_cargo_version();
        assert!(version.is_ok());
        let v = version.unwrap();
        assert!(v.starts_with("cargo"));
    }

    #[test]
    fn get_ci_branch_returns_none_locally() {
        // When not in CI, should return None
        if !is_ci() {
            assert!(get_ci_branch().is_none());
        }
    }

    #[test]
    fn get_ci_commit_sha_returns_none_locally() {
        // When not in CI, should return None
        if !is_ci() {
            assert!(get_ci_commit_sha().is_none());
        }
    }

    #[test]
    fn environment_info_serialization() {
        let info = EnvironmentInfo {
            ci_environment: CiEnvironment::GitHubActions,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            rust_version: "1.70.0".to_string(),
            cargo_version: "1.70.0".to_string(),
            env_vars: BTreeMap::new(),
            collected_at: Utc::now(),
        };

        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("\"ci_environment\":\"GitHubActions\""));
    }
}