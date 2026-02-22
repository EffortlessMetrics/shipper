//! Registry API client for shipper.
//!
//! This crate provides API clients for interacting with crate registries
//! like crates.io, supporting version existence checks and visibility polling.
//!
//! # Example
//!
//! ```
//! use shipper_registry::{RegistryClient, CrateInfo};
//!
//! let client = RegistryClient::new("https://crates.io");
//!
//! // Check if a crate exists
//! let exists = client.crate_exists("serde").unwrap_or(false);
//!
//! // Check if a specific version exists
//! let version_exists = client.version_exists("serde", "1.0.0").unwrap_or(false);
//! ```

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Default API endpoint for crates.io
pub const CRATES_IO_API: &str = "https://crates.io";

/// Default timeout for API requests
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Default user agent for API requests
pub const USER_AGENT: &str = concat!("shipper/", env!("CARGO_PKG_VERSION"));

/// Registry API client
#[derive(Debug, Clone)]
pub struct RegistryClient {
    base_url: String,
    timeout: Duration,
    client: reqwest::blocking::Client,
}

impl RegistryClient {
    /// Create a new registry client for the given base URL
    pub fn new(base_url: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            client,
        }
    }

    /// Create a client for crates.io
    pub fn crates_io() -> Self {
        Self::new(CRATES_IO_API)
    }

    /// Set the request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        self
    }

    /// Check if a crate exists in the registry
    pub fn crate_exists(&self, name: &str) -> Result<bool> {
        let url = format!("{}/api/v1/crates/{}", self.base_url, name);

        let response = self
            .client
            .get(&url)
            .send()
            .context("failed to send request to registry")?;

        match response.status() {
            reqwest::StatusCode::OK => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            status => Err(anyhow::anyhow!("unexpected status code: {}", status)),
        }
    }

    /// Check if a specific version of a crate exists
    pub fn version_exists(&self, name: &str, version: &str) -> Result<bool> {
        let url = format!("{}/api/v1/crates/{}/{}", self.base_url, name, version);

        let response = self
            .client
            .get(&url)
            .send()
            .context("failed to send request to registry")?;

        match response.status() {
            reqwest::StatusCode::OK => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            status => Err(anyhow::anyhow!("unexpected status code: {}", status)),
        }
    }

    /// Get crate information
    pub fn get_crate_info(&self, name: &str) -> Result<Option<CrateInfo>> {
        let url = format!("{}/api/v1/crates/{}", self.base_url, name);

        let response = self
            .client
            .get(&url)
            .send()
            .context("failed to send request to registry")?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "unexpected status code: {}",
                response.status()
            ));
        }

        let crate_response: CrateResponse =
            response.json().context("failed to parse crate response")?;

        Ok(Some(CrateInfo {
            name: crate_response.crate_data.name,
            newest_version: crate_response.crate_data.newest_version,
            created_at: crate_response.crate_data.created_at,
            updated_at: crate_response.crate_data.updated_at,
        }))
    }

    fn fetch_owners_with_token(
        &self,
        name: &str,
        token: Option<&str>,
    ) -> Result<Option<OwnersResponse>> {
        let url = format!("{}/api/v1/crates/{}/owners", self.base_url, name);
        let mut request = self.client.get(&url);
        if let Some(token) = token {
            request = request.header("Authorization", token);
        }

        let response = request.send().context("failed to query owners")?;
        match response.status() {
            reqwest::StatusCode::OK => {
                let owners_response: OwnersResponse =
                    response.json().context("failed to parse owners response")?;
                Ok(Some(owners_response))
            }
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::UNAUTHORIZED => {
                Err(anyhow::anyhow!(
                    "forbidden when querying owners; token may be invalid or missing required scope"
                ))
            }
            status => Err(anyhow::anyhow!(
                "unexpected status while querying owners: {status}"
            )),
        }
    }

    /// Get the list of owners for a crate.
    pub fn get_owners(&self, name: &str) -> Result<Vec<Owner>> {
        let owners_response = self
            .fetch_owners_with_token(name, None)?
            .unwrap_or_default();
        Ok(owners_response
            .users
            .into_iter()
            .map(|owner| Owner {
                login: owner.login,
                name: owner.name,
                avatar: owner.avatar,
            })
            .collect())
    }

    /// List owners for a crate with token-aware lookup.
    pub fn list_owners(&self, name: &str, token: &str) -> Result<OwnersResponse> {
        self.fetch_owners_with_token(name, Some(token))?
            .ok_or_else(|| anyhow::anyhow!("crate not found when querying owners: {name}"))
    }

    /// Check if a user is an owner of a crate
    pub fn is_owner(&self, name: &str, username: &str) -> Result<bool> {
        let owners = self.get_owners(name)?;
        Ok(owners.iter().any(|o| o.login == username))
    }

    /// Check if a version exists in sparse-index metadata.
    pub fn is_version_visible_in_sparse_index(
        &self,
        index_base: &str,
        name: &str,
        version: &str,
    ) -> Result<bool> {
        let content = self.fetch_sparse_index_file(index_base, name)?;
        parse_version_from_sparse_index(&content, version)
    }

    /// Fetch sparse-index content for a crate.
    pub fn fetch_sparse_index_file(&self, index_base: &str, name: &str) -> Result<String> {
        let index_base = index_base.trim_end_matches('/');
        let index_path = sparse_index_path(name);
        let url = format!("{}/{}", index_base, index_path);

        let response = self
            .client
            .get(&url)
            .send()
            .context("index request failed")?;

        match response.status() {
            reqwest::StatusCode::OK => response
                .text()
                .context("failed to read index response body"),
            reqwest::StatusCode::NOT_FOUND => Err(anyhow::anyhow!("index file not found: {url}")),
            status => Err(anyhow::anyhow!(
                "unexpected status while fetching index: {status}"
            )),
        }
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// Crate information from the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfo {
    /// Crate name
    pub name: String,
    /// Newest version available
    pub newest_version: String,
    /// When the crate was created
    pub created_at: String,
    /// When the crate was last updated
    pub updated_at: String,
}

/// Owner information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnersApiUser {
    /// Optional owner ID (not guaranteed in all registry responses)
    pub id: Option<u64>,
    /// Owner's login/username
    pub login: String,
    /// Owner's display name
    pub name: Option<String>,
    /// Owner's avatar URL
    pub avatar: Option<String>,
}

/// Owner information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Owner {
    /// Owner's login/username
    pub login: String,
    /// Owner's display name
    pub name: Option<String>,
    /// Owner's avatar URL
    pub avatar: Option<String>,
}

/// Response from the crate API
#[derive(Debug, Deserialize)]
struct CrateResponse {
    #[serde(rename = "crate")]
    crate_data: CrateData,
}

/// Crate data from API
#[derive(Debug, Deserialize)]
struct CrateData {
    name: String,
    newest_version: String,
    created_at: String,
    updated_at: String,
}

/// Response from the owners API
#[derive(Debug, Default, Deserialize)]
pub struct OwnersResponse {
    pub users: Vec<OwnersApiUser>,
}

fn parse_version_from_sparse_index(content: &str, version: &str) -> Result<bool> {
    #[derive(Deserialize)]
    struct IndexVersion {
        vers: String,
    }

    Ok(content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<IndexVersion>(line).ok())
        .any(|value| value.vers == version))
}

/// Compute the sparse-index path for a crate name.
pub fn sparse_index_path(crate_name: &str) -> String {
    let lower = crate_name.to_ascii_lowercase();
    match lower.len() {
        1 => format!("1/{}", lower),
        2 => format!("2/{}", lower),
        3 => format!("3/{}/{}", &lower[..1], lower),
        _ => format!("{}/{}/{}", &lower[..2], &lower[2..4], lower),
    }
}

/// Check if a crate version is visible on the registry
pub fn is_version_visible(base_url: &str, name: &str, version: &str) -> Result<bool> {
    let client = RegistryClient::new(base_url);
    client.version_exists(name, version)
}

/// Check if a crate exists on the registry
pub fn is_crate_visible(base_url: &str, name: &str) -> Result<bool> {
    let client = RegistryClient::new(base_url);
    client.crate_exists(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_creation() {
        let client = RegistryClient::crates_io();
        assert_eq!(client.base_url(), "https://crates.io");
    }

    #[test]
    fn client_with_custom_url() {
        let client = RegistryClient::new("https://custom.registry.io/");
        assert_eq!(client.base_url(), "https://custom.registry.io");
    }

    #[test]
    fn client_with_timeout() {
        let client = RegistryClient::crates_io().with_timeout(Duration::from_secs(60));
        assert_eq!(client.timeout, Duration::from_secs(60));
    }

    #[test]
    fn crate_info_serialization() {
        let info = CrateInfo {
            name: "test-crate".to_string(),
            newest_version: "1.0.0".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-02T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("\"name\":\"test-crate\""));
    }

    #[test]
    fn owner_serialization() {
        let owner = Owner {
            login: "testuser".to_string(),
            name: Some("Test User".to_string()),
            avatar: Some("https://example.com/avatar.png".to_string()),
        };

        let json = serde_json::to_string(&owner).expect("serialize");
        assert!(json.contains("\"login\":\"testuser\""));
    }

    #[derive(Debug, Deserialize)]
    struct VersionsResponse {
        versions: Vec<Version>,
    }

    #[derive(Debug, Deserialize)]
    struct Version {
        num: String,
    }

    #[test]
    fn versions_response_parsing() {
        let json = r#"{"versions":[{"num":"1.0.0"},{"num":"0.9.0"}]}"#;
        let response: VersionsResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(response.versions.len(), 2);
        assert_eq!(response.versions[0].num, "1.0.0");
    }

    #[test]
    fn crate_response_parsing() {
        let json = r#"{
            "crate": {
                "name": "serde",
                "newest_version": "1.0.190",
                "created_at": "2017-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z"
            }
        }"#;
        let response: CrateResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(response.crate_data.name, "serde");
        assert_eq!(response.crate_data.newest_version, "1.0.190");
    }

    #[test]
    fn owners_response_parsing() {
        let json = r#"{
            "users": [
                {"login": "user1", "name": "User One", "avatar": null},
                {"login": "user2", "name": null, "avatar": "https://example.com/avatar.png"}
            ]
        }"#;
        let response: OwnersResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(response.users.len(), 2);
        assert_eq!(response.users[0].login, "user1");
        assert_eq!(
            response.users[1].avatar,
            Some("https://example.com/avatar.png".to_string())
        );
    }

    #[test]
    fn user_agent_includes_version() {
        assert!(USER_AGENT.starts_with("shipper/"));
        assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
    }
}
