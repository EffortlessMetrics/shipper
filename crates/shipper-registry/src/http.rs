//! Lightweight HTTP-only registry client.
//!
//! This module provides [`HttpRegistryClient`] — a thin reqwest wrapper that
//! takes a bare base-URL string. Intended for callers that do not have a full
//! [`shipper_types::Registry`] handy (e.g. the parallel engine helper crate).
//! For the complete registry surface, use the [`crate::HttpRegistryClient`] from
//! the [`crate::context`] module.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use shipper_types::Registry;

use crate::{
    CRATES_IO_API, DEFAULT_TIMEOUT_SECS, RegistryAuthority, RegistryPolicy, USER_AGENT,
    ValidatedRegistry, authorities_share_trusted_domain, sparse_index_path,
};

/// Lightweight HTTP registry client that operates on a raw base-URL.
///
/// Use [`crate::HttpRegistryClient`] for the full `Registry`-aware client with
/// sparse-index visibility checks, readiness backoff, etc.
#[derive(Debug, Clone)]
pub struct HttpRegistryClient {
    base_url: String,
    timeout: Duration,
    client: Option<reqwest::blocking::Client>,
    cache_dir: Option<std::path::PathBuf>,
    policy: RegistryPolicy,
    validated_authority: Option<RegistryAuthority>,
    resolved_host: Option<String>,
    resolved_addrs: Vec<SocketAddr>,
    validation_error: Option<String>,
}

impl HttpRegistryClient {
    /// Create a new registry client for the given base URL
    pub fn new(base_url: &str) -> Self {
        let policy = RegistryPolicy::secure();
        match Self::try_with_policy(base_url, policy) {
            Ok(client) => client,
            Err(error) => Self {
                base_url: redacted_base_url(base_url),
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                client: None,
                cache_dir: None,
                policy,
                validated_authority: None,
                resolved_host: None,
                resolved_addrs: Vec::new(),
                validation_error: Some(error.to_string()),
            },
        }
    }

    /// Construct a client with explicit registry trust choices.
    pub fn with_policy(base_url: &str, policy: RegistryPolicy) -> Result<Self> {
        Self::try_with_policy(base_url, policy)
    }

    fn try_with_policy(base_url: &str, policy: RegistryPolicy) -> Result<Self> {
        let identity = Registry {
            name: "http-client".to_string(),
            api_base: base_url.to_string(),
            index_base: Some(base_url.to_string()),
        };
        let validated = ValidatedRegistry::new(identity, policy)?;
        let resolved_host = validated
            .api_base()
            .host_str()
            .ok_or_else(|| anyhow!("validated api_base URL has no host"))?
            .to_string();
        let resolved_addrs = validated.approved_socket_addrs(validated.api_base(), "api_base")?;

        Ok(Self {
            base_url: validated
                .api_base()
                .as_str()
                .trim_end_matches('/')
                .to_string(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            client: Some(build_client(
                Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                &[(resolved_host.as_str(), resolved_addrs.as_slice())],
            )?),
            cache_dir: None,
            policy,
            validated_authority: Some(validated.credential_authority().clone()),
            resolved_host: Some(resolved_host),
            resolved_addrs,
            validation_error: None,
        })
    }

    /// Set the cache directory for sparse index fragments
    pub fn with_cache_dir(mut self, cache_dir: std::path::PathBuf) -> Self {
        self.cache_dir = Some(cache_dir);
        self
    }

    /// Create a client for crates.io
    pub fn crates_io() -> Self {
        Self::new(CRATES_IO_API)
    }

    /// Set the request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        if self.validation_error.is_none() {
            match build_client(
                timeout,
                &[(
                    self.resolved_host.as_deref().unwrap_or_default(),
                    self.resolved_addrs.as_slice(),
                )],
            ) {
                Ok(client) => self.client = Some(client),
                Err(error) => {
                    self.client = None;
                    self.validation_error = Some(error.to_string());
                }
            }
        }
        self
    }

    fn ensure_validated(&self) -> Result<()> {
        self.validation_error
            .as_deref()
            .map_or(Ok(()), |error| Err(anyhow!("{error}")))
    }

    fn http_client(&self) -> Result<&reqwest::blocking::Client> {
        self.ensure_validated()?;
        self.client
            .as_ref()
            .ok_or_else(|| anyhow!("registry HTTP client is unavailable"))
    }

    /// Check if a crate exists in the registry
    pub fn crate_exists(&self, name: &str) -> Result<bool> {
        let url = format!("{}/api/v1/crates/{}", self.base_url, name);

        let response = self
            .http_client()?
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
            .http_client()?
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
            .http_client()?
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
        let mut request = self.http_client()?.get(&url);
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
        Ok(shipper_sparse_index::contains_version(&content, version))
    }

    /// Fetch sparse-index content for a crate.
    pub fn fetch_sparse_index_file(&self, index_base: &str, name: &str) -> Result<String> {
        self.ensure_validated()?;
        let index_identity = Registry {
            name: "http-client-index".to_string(),
            api_base: index_base.to_string(),
            index_base: Some(index_base.to_string()),
        };
        let validated_index = ValidatedRegistry::new(index_identity, self.policy)
            .context("invalid sparse index destination")?;
        if !self.validated_authority.as_ref().is_some_and(|authority| {
            authorities_share_trusted_domain(authority, validated_index.credential_authority())
        }) {
            return Err(anyhow!(
                "sparse index destination authority must match the registry client authority"
            ));
        }
        let index_base = validated_index.index_base().as_str().trim_end_matches('/');
        let index_path = sparse_index_path(name);
        let url = format!("{}/{}", index_base, index_path);

        let cache_file = self.cache_dir.as_ref().map(|d| d.join(&index_path));
        let etag_file = cache_file.as_ref().map(|f| f.with_extension("etag"));

        let mut request = self.http_client()?.get(&url);

        if let Some(ref path) = etag_file
            && let Ok(etag) = std::fs::read_to_string(path)
        {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag.trim());
        }

        // The API client pins the API hostname. Index traffic may use a
        // different, but explicitly trusted, hostname, so it needs its own
        // pinned client as well. Reusing the API client here would let the
        // index hostname resolve at connection time and reopen a DNS-rebind
        // window after validation.
        let index_host = validated_index
            .index_base()
            .host_str()
            .ok_or_else(|| anyhow!("validated index URL has no host"))?;
        let index_addrs =
            validated_index.approved_socket_addrs(validated_index.index_base(), "index_base")?;
        let index_client = build_client(self.timeout, &[(index_host, index_addrs.as_slice())])?;
        let response = request.build().context("failed to build index request")?;
        let response = index_client
            .execute(response)
            .context("index request failed")?;

        match response.status() {
            reqwest::StatusCode::OK => {
                let etag = response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|h| h.to_str().ok())
                    .map(|s| s.to_string());
                let content = response
                    .text()
                    .context("failed to read index response body")?;

                if let Some(ref path) = cache_file {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(path, &content);
                    if let (Some(ref etag_val), Some(etag_path)) = (etag, etag_file) {
                        let _ = std::fs::write(etag_path, etag_val);
                    }
                }
                Ok(content)
            }
            reqwest::StatusCode::NOT_MODIFIED => {
                if let Some(ref path) = cache_file {
                    std::fs::read_to_string(path).context("failed to read cached index file")
                } else {
                    Err(anyhow::anyhow!(
                        "received 304 Not Modified but no cache file available"
                    ))
                }
            }
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

fn build_client(
    timeout: Duration,
    resolved_hosts: &[(&str, &[SocketAddr])],
) -> Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none());
    for (host, addrs) in resolved_hosts {
        builder = builder.resolve_to_addrs(host, addrs);
    }
    builder
        .build()
        .context("failed to build registry HTTP client")
}

fn redacted_base_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return "<invalid registry URL>".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
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
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OwnersResponse {
    pub users: Vec<OwnersApiUser>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CRATES_IO_API, is_crate_visible, is_version_visible};

    fn test_client(base_url: &str) -> HttpRegistryClient {
        HttpRegistryClient::with_policy(base_url, RegistryPolicy::rehearsal())
            .expect("valid rehearsal registry URL")
    }

    #[test]
    fn client_creation() {
        let client = HttpRegistryClient::crates_io();
        assert_eq!(client.base_url(), "https://crates.io");
    }

    #[test]
    fn client_with_custom_url() {
        let client = test_client("https://127.0.0.1/");
        assert_eq!(client.base_url(), "https://127.0.0.1");
    }

    #[test]
    fn invalid_destination_fails_before_any_credentialed_request() {
        let client = HttpRegistryClient::new("https://user:fixture-secret@registry.example");
        assert!(!client.base_url().contains("fixture-secret"));
        let error = client.list_owners("demo", "fixture-token").unwrap_err();

        assert!(!error.to_string().contains("fixture-secret"));
        assert!(!error.to_string().contains("fixture-token"));
    }

    #[test]
    fn dns_name_uses_only_policy_approved_pinned_addresses() {
        use tiny_http::{Response, Server, StatusCode};

        let server = Server::http("127.0.0.1:0").expect("mock server");
        let direct_base = format!("http://{}", server.server_addr());
        let hostname_base = direct_base.replace("127.0.0.1", "localhost");
        let handle = std::thread::spawn(move || {
            let request = server.recv().expect("request");
            request
                .respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let client = test_client(&hostname_base);
        assert!(!client.resolved_addrs.is_empty());
        assert!(
            client
                .resolved_addrs
                .iter()
                .all(|address| address.ip().is_loopback())
        );
        assert!(client.crate_exists("demo").expect("request"));
        handle.join().expect("server thread");
    }

    #[test]
    fn redirects_are_not_followed_to_a_second_request() {
        use tiny_http::{Header, Response, Server, StatusCode};

        let server = Server::http("127.0.0.1:0").expect("mock server");
        let base = format!("http://{}", server.server_addr());
        let location = format!("{base}/redirected");
        let handle = std::thread::spawn(move || {
            let request = server.recv().expect("initial request");
            let response = Response::empty(StatusCode(302))
                .with_header(Header::from_bytes("Location", location).expect("location header"));
            request.respond(response).expect("redirect response");
            assert!(
                server
                    .recv_timeout(Duration::from_millis(200))
                    .expect("redirect observation")
                    .is_none(),
                "redirect policy must prevent a second request"
            );
        });

        let error = test_client(&base)
            .crate_exists("demo")
            .expect_err("302 must not be followed");
        assert!(error.to_string().contains("unexpected status code: 302"));
        handle.join().expect("server thread");
    }

    #[test]
    fn sparse_index_must_match_client_authority() {
        let client = test_client("http://127.0.0.1:1");
        let error = client
            .fetch_sparse_index_file("http://127.0.0.1:2", "demo")
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("must match the registry client authority")
        );
    }

    #[test]
    fn private_destination_requires_explicit_policy() {
        assert!(
            HttpRegistryClient::with_policy("https://10.0.0.5", RegistryPolicy::secure(),).is_err()
        );
        assert!(
            HttpRegistryClient::with_policy(
                "https://10.0.0.5",
                RegistryPolicy::secure().with_private(true),
            )
            .is_ok()
        );
    }

    #[test]
    fn client_with_timeout() {
        let client = HttpRegistryClient::crates_io().with_timeout(Duration::from_mins(1));
        assert_eq!(client.timeout, Duration::from_mins(1));
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

    #[test]
    fn test_sparse_index_caching() {
        use tiny_http::{Header, Response, Server, StatusCode};

        let server = Server::http("127.0.0.1:0").expect("server");
        let base_url = format!("http://{}", server.server_addr());

        let td = tempfile::tempdir().expect("tempdir");
        let cache_dir = td.path().to_path_buf();

        let handle = std::thread::spawn({
            let _base_url = base_url.clone();
            move || {
                // First request: return 200 OK with ETag
                let req = server.recv().expect("request 1");
                assert_eq!(req.url(), "/de/mo/demo");
                let resp = Response::from_string("{\"vers\":\"0.1.0\"}")
                    .with_status_code(StatusCode(200))
                    .with_header(Header::from_bytes("ETag", "W/\"123\"").unwrap());
                req.respond(resp).expect("respond 1");

                // Second request: expect If-None-Match and return 304
                let req = server.recv().expect("request 2");
                assert_eq!(req.url(), "/de/mo/demo");
                let etag_header = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("If-None-Match"))
                    .expect("missing If-None-Match");
                assert_eq!(etag_header.value.as_str(), "W/\"123\"");

                let resp = Response::from_string("").with_status_code(StatusCode(304));
                req.respond(resp).expect("respond 2");
            }
        });

        let client = test_client(&base_url).with_cache_dir(cache_dir);

        // First call: should fetch and cache
        let content1 = client
            .fetch_sparse_index_file(&base_url, "demo")
            .expect("fetch 1");
        assert_eq!(content1, "{\"vers\":\"0.1.0\"}");

        // Second call: should use 304 and read from cache
        let content2 = client
            .fetch_sparse_index_file(&base_url, "demo")
            .expect("fetch 2");
        assert_eq!(content2, "{\"vers\":\"0.1.0\"}");

        handle.join().expect("join");
    }

    // ── Helper: spin up a tiny_http mock server ──────────────────────

    fn mock_server() -> (tiny_http::Server, String) {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("mock server");
        let base = format!("http://{}", server.server_addr());
        (server, base)
    }

    fn respond(req: tiny_http::Request, status: u16, body: &str) {
        let resp =
            tiny_http::Response::from_string(body).with_status_code(tiny_http::StatusCode(status));
        req.respond(resp).expect("respond");
    }

    // ── URL construction ─────────────────────────────────────────────

    #[test]
    fn url_multiple_trailing_slashes_stripped() {
        let client = test_client("https://example.com///");
        assert_eq!(client.base_url(), "https://example.com");
    }

    #[test]
    fn url_no_trailing_slash_unchanged() {
        let client = test_client("https://example.com");
        assert_eq!(client.base_url(), "https://example.com");
    }

    #[test]
    fn default_timeout_is_30s() {
        let client = HttpRegistryClient::crates_io();
        assert_eq!(client.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    }

    #[test]
    fn with_cache_dir_sets_cache() {
        let td = tempfile::tempdir().expect("tempdir");
        let client = HttpRegistryClient::crates_io().with_cache_dir(td.path().to_path_buf());
        assert_eq!(client.cache_dir, Some(td.path().to_path_buf()));
    }

    // ── crate_exists (mock) ──────────────────────────────────────────

    #[test]
    fn crate_exists_returns_true_on_200() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            let req = server.recv().expect("request");
            assert_eq!(req.url(), "/api/v1/crates/serde");
            respond(req, 200, r#"{"crate":{}}"#);
        });
        let client = test_client(&base);
        assert!(client.crate_exists("serde").expect("ok"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_returns_false_on_404() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        assert!(!client.crate_exists("nonexistent").expect("ok"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_returns_error_on_500() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 500, "");
        });
        let client = test_client(&base);
        let err = client.crate_exists("bad").unwrap_err();
        assert!(err.to_string().contains("unexpected status code"));
        handle.join().expect("join");
    }

    // ── version_exists (mock) ────────────────────────────────────────

    #[test]
    fn version_exists_returns_true_on_200() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            let req = server.recv().expect("req");
            assert_eq!(req.url(), "/api/v1/crates/serde/1.0.0");
            respond(req, 200, "{}");
        });
        let client = test_client(&base);
        assert!(client.version_exists("serde", "1.0.0").expect("ok"));
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_returns_false_on_404() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        assert!(!client.version_exists("serde", "99.0.0").expect("ok"));
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_returns_error_on_503() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 503, "");
        });
        let client = test_client(&base);
        let err = client.version_exists("x", "0.1.0").unwrap_err();
        assert!(err.to_string().contains("unexpected status code"));
        handle.join().expect("join");
    }

    // ── get_crate_info (mock) ────────────────────────────────────────

    #[test]
    fn get_crate_info_returns_some_on_200() {
        let (server, base) = mock_server();
        let body = r#"{
            "crate": {
                "name": "demo",
                "newest_version": "2.0.0",
                "created_at": "2023-01-01T00:00:00Z",
                "updated_at": "2024-06-01T00:00:00Z"
            }
        }"#;
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 200, body);
        });
        let client = test_client(&base);
        let info = client.get_crate_info("demo").expect("ok").expect("Some");
        assert_eq!(info.name, "demo");
        assert_eq!(info.newest_version, "2.0.0");
        assert_eq!(info.created_at, "2023-01-01T00:00:00Z");
        assert_eq!(info.updated_at, "2024-06-01T00:00:00Z");
        handle.join().expect("join");
    }

    #[test]
    fn get_crate_info_returns_none_on_404() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        assert!(client.get_crate_info("nope").expect("ok").is_none());
        handle.join().expect("join");
    }

    #[test]
    fn get_crate_info_returns_error_on_500() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 500, "");
        });
        let client = test_client(&base);
        let err = client.get_crate_info("bad").unwrap_err();
        assert!(err.to_string().contains("unexpected status code"));
        handle.join().expect("join");
    }

    #[test]
    fn get_crate_info_returns_error_on_invalid_json() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 200, "NOT JSON");
        });
        let client = test_client(&base);
        let err = client.get_crate_info("bad").unwrap_err();
        assert!(err.to_string().contains("failed to parse crate response"));
        handle.join().expect("join");
    }

    // ── owners endpoints (mock) ──────────────────────────────────────

    #[test]
    fn get_owners_returns_owners_on_200() {
        let (server, base) = mock_server();
        let body = r#"{"users":[{"login":"alice","name":"Alice","avatar":null}]}"#;
        let handle = std::thread::spawn(move || {
            let req = server.recv().expect("req");
            assert_eq!(req.url(), "/api/v1/crates/demo/owners");
            respond(req, 200, body);
        });
        let client = test_client(&base);
        let owners = client.get_owners("demo").expect("ok");
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].login, "alice");
        handle.join().expect("join");
    }

    #[test]
    fn get_owners_returns_empty_on_404() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        let owners = client.get_owners("nonexistent").expect("ok");
        assert!(owners.is_empty());
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_sends_auth_header() {
        let (server, base) = mock_server();
        let body = r#"{"users":[{"login":"bob","name":null,"avatar":null}]}"#;
        let handle = std::thread::spawn(move || {
            let req = server.recv().expect("req");
            let auth = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .expect("missing Authorization");
            assert_eq!(auth.value.as_str(), "my-token");
            respond(req, 200, body);
        });
        let client = test_client(&base);
        let resp = client.list_owners("demo", "my-token").expect("ok");
        assert_eq!(resp.users.len(), 1);
        assert_eq!(resp.users[0].login, "bob");
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_returns_error_on_403() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 403, "");
        });
        let client = test_client(&base);
        let err = client.list_owners("demo", "bad-token").unwrap_err();
        assert!(err.to_string().contains("forbidden"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_returns_error_on_401() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 401, "");
        });
        let client = test_client(&base);
        let err = client.list_owners("demo", "expired").unwrap_err();
        assert!(err.to_string().contains("forbidden"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_returns_error_on_crate_not_found() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        let err = client.list_owners("nope", "token").unwrap_err();
        assert!(err.to_string().contains("crate not found"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_returns_error_on_unexpected_status() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 502, "");
        });
        let client = test_client(&base);
        let err = client.list_owners("demo", "tok").unwrap_err();
        assert!(err.to_string().contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn is_owner_returns_true_for_matching_user() {
        let (server, base) = mock_server();
        let body = r#"{"users":[{"login":"carol","name":null,"avatar":null}]}"#;
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 200, body);
        });
        let client = test_client(&base);
        assert!(client.is_owner("demo", "carol").expect("ok"));
        handle.join().expect("join");
    }

    #[test]
    fn is_owner_returns_false_for_non_matching_user() {
        let (server, base) = mock_server();
        let body = r#"{"users":[{"login":"carol","name":null,"avatar":null}]}"#;
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 200, body);
        });
        let client = test_client(&base);
        assert!(!client.is_owner("demo", "dave").expect("ok"));
        handle.join().expect("join");
    }

    // ── sparse index (mock) ──────────────────────────────────────────

    #[test]
    fn fetch_sparse_index_not_found() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        let err = client.fetch_sparse_index_file(&base, "xy").unwrap_err();
        assert!(err.to_string().contains("index file not found"));
        handle.join().expect("join");
    }

    #[test]
    fn fetch_sparse_index_unexpected_status() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 502, "");
        });
        let client = test_client(&base);
        let err = client.fetch_sparse_index_file(&base, "xy").unwrap_err();
        assert!(err.to_string().contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn fetch_sparse_index_304_without_cache_errors() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 304, "");
        });
        // No cache_dir set
        let client = test_client(&base);
        let err = client.fetch_sparse_index_file(&base, "ab").unwrap_err();
        assert!(err.to_string().contains("304 Not Modified"));
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_in_sparse_index_with_mock() {
        let (server, base) = mock_server();
        let body = "{\"name\":\"demo\",\"vers\":\"0.1.0\",\"deps\":[]}\n\
                    {\"name\":\"demo\",\"vers\":\"0.2.0\",\"deps\":[]}";
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 200, body);
        });
        let client = test_client(&base);
        assert!(
            client
                .is_version_visible_in_sparse_index(&base, "demo", "0.1.0")
                .expect("ok")
        );
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_in_sparse_index_uses_unicode_path() {
        let (server, base) = mock_server();
        let body = "{\"name\":\"café\",\"vers\":\"0.1.0\",\"deps\":[]}";
        let handle = std::thread::spawn(move || {
            let request = server.recv().expect("request");
            assert_eq!(request.url(), "/ca/f%C3%A9/caf%C3%A9");
            respond(request, 200, body);
        });
        let client = test_client(&base);
        assert!(
            client
                .is_version_visible_in_sparse_index(&base, "café", "0.1.0")
                .expect("ok")
        );
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_in_sparse_index_returns_false_for_missing_version() {
        let (server, base) = mock_server();
        let body = "{\"name\":\"demo\",\"vers\":\"0.1.0\",\"deps\":[]}";
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 200, body);
        });
        let client = test_client(&base);
        assert!(
            !client
                .is_version_visible_in_sparse_index(&base, "demo", "9.9.9")
                .expect("ok")
        );
        handle.join().expect("join");
    }

    // ── convenience functions ────────────────────────────────────────

    #[test]
    fn is_version_visible_rejects_loopback_without_explicit_policy() {
        let error = is_version_visible("http://127.0.0.1:1", "serde", "1.0.0").unwrap_err();
        assert!(error.to_string().contains("loopback"));
    }

    #[test]
    fn is_crate_visible_rejects_loopback_without_explicit_policy() {
        let error = is_crate_visible("http://127.0.0.1:1", "serde").unwrap_err();
        assert!(error.to_string().contains("loopback"));
    }

    // ── timeout handling ─────────────────────────────────────────────

    #[test]
    fn timeout_triggers_on_slow_server() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            let req = server.recv().expect("req");
            // Sleep longer than the client timeout
            std::thread::sleep(Duration::from_secs(3));
            let _ = req.respond(tiny_http::Response::from_string("{}"));
        });
        let client = test_client(&base).with_timeout(Duration::from_millis(200));
        let result = client.crate_exists("slow");
        assert!(result.is_err());
        handle.join().expect("join");
    }

    // ── connection error ─────────────────────────────────────────────

    #[test]
    fn crate_exists_handles_connection_refused() {
        // Use a port that is very unlikely to be listening
        let client = test_client("http://127.0.0.1:1");
        let result = client.crate_exists("anything");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("failed to send request")
        );
    }

    // ── serialization round-trips ────────────────────────────────────

    #[test]
    fn crate_info_roundtrip() {
        let info = CrateInfo {
            name: "foo".to_string(),
            newest_version: "3.2.1".to_string(),
            created_at: "2020-01-01T00:00:00Z".to_string(),
            updated_at: "2025-06-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&info).expect("ser");
        let back: CrateInfo = serde_json::from_str(&json).expect("de");
        assert_eq!(back.name, "foo");
        assert_eq!(back.newest_version, "3.2.1");
    }

    #[test]
    fn owner_roundtrip_with_optional_fields() {
        let owner = Owner {
            login: "user".to_string(),
            name: None,
            avatar: None,
        };
        let json = serde_json::to_string(&owner).expect("ser");
        let back: Owner = serde_json::from_str(&json).expect("de");
        assert_eq!(back.login, "user");
        assert!(back.name.is_none());
        assert!(back.avatar.is_none());
    }

    #[test]
    fn owners_api_user_optional_id() {
        let json = r#"{"login":"alice","name":null,"avatar":null}"#;
        let user: OwnersApiUser = serde_json::from_str(json).expect("de");
        assert!(user.id.is_none());
        assert_eq!(user.login, "alice");
    }

    #[test]
    fn owners_api_user_with_id() {
        let json = r#"{"id":42,"login":"bob","name":"Bob","avatar":"http://a.png"}"#;
        let user: OwnersApiUser = serde_json::from_str(json).expect("de");
        assert_eq!(user.id, Some(42));
        assert_eq!(user.login, "bob");
    }

    #[test]
    fn owners_response_default_is_empty() {
        let resp = OwnersResponse::default();
        assert!(resp.users.is_empty());
    }

    // ── sparse_index_path delegation ─────────────────────────────────

    #[test]
    fn sparse_index_path_short_crate() {
        assert_eq!(sparse_index_path("a"), "1/a");
        assert_eq!(sparse_index_path("ab"), "2/ab");
    }

    #[test]
    fn sparse_index_path_three_char() {
        assert_eq!(sparse_index_path("abc"), "3/a/abc");
    }

    #[test]
    fn sparse_index_path_four_plus_char() {
        assert_eq!(sparse_index_path("demo"), "de/mo/demo");
        assert_eq!(sparse_index_path("serde"), "se/rd/serde");
    }

    #[test]
    fn sparse_index_path_delegation_handles_unicode() {
        assert_eq!(crate::sparse_index_path("café"), "ca/fé/café");
    }

    // ── constants ────────────────────────────────────────────────────

    #[test]
    fn crates_io_api_constant() {
        assert_eq!(CRATES_IO_API, "https://crates.io");
    }

    #[test]
    fn default_timeout_constant() {
        assert_eq!(DEFAULT_TIMEOUT_SECS, 30);
    }

    // ── insta snapshot tests ─────────────────────────────────────────

    #[test]
    fn snapshot_crate_info() {
        let info = CrateInfo {
            name: "my-crate".to_string(),
            newest_version: "1.2.3".to_string(),
            created_at: "2024-01-15T10:30:00Z".to_string(),
            updated_at: "2024-06-20T14:00:00Z".to_string(),
        };
        insta::assert_yaml_snapshot!("crate_info", info);
    }

    #[test]
    fn snapshot_owner_all_fields() {
        let owner = Owner {
            login: "alice".to_string(),
            name: Some("Alice Smith".to_string()),
            avatar: Some("https://example.com/alice.png".to_string()),
        };
        insta::assert_yaml_snapshot!("owner_all_fields", owner);
    }

    #[test]
    fn snapshot_owner_minimal() {
        let owner = Owner {
            login: "bot-user".to_string(),
            name: None,
            avatar: None,
        };
        insta::assert_yaml_snapshot!("owner_minimal", owner);
    }

    #[test]
    fn snapshot_owners_api_user_with_id() {
        let user = OwnersApiUser {
            id: Some(42),
            login: "bob".to_string(),
            name: Some("Bob Jones".to_string()),
            avatar: Some("https://example.com/bob.png".to_string()),
        };
        insta::assert_yaml_snapshot!("owners_api_user_with_id", user);
    }

    #[test]
    fn snapshot_owners_api_user_without_id() {
        let user = OwnersApiUser {
            id: None,
            login: "team:core".to_string(),
            name: None,
            avatar: None,
        };
        insta::assert_yaml_snapshot!("owners_api_user_without_id", user);
    }

    #[test]
    fn snapshot_owners_response_multiple() {
        let resp = OwnersResponse {
            users: vec![
                OwnersApiUser {
                    id: Some(1),
                    login: "alice".to_string(),
                    name: Some("Alice".to_string()),
                    avatar: None,
                },
                OwnersApiUser {
                    id: Some(2),
                    login: "bob".to_string(),
                    name: None,
                    avatar: Some("https://example.com/bob.png".to_string()),
                },
            ],
        };
        insta::assert_yaml_snapshot!("owners_response_multiple", resp);
    }

    #[test]
    fn snapshot_owners_response_empty() {
        let resp = OwnersResponse::default();
        insta::assert_yaml_snapshot!("owners_response_empty", resp);
    }

    #[test]
    fn snapshot_url_construction_crate() {
        let client = test_client("https://crates.io");
        let url = format!("{}/api/v1/crates/{}", client.base_url(), "my-crate");
        insta::assert_snapshot!("url_crate", url);
    }

    #[test]
    fn snapshot_url_construction_version() {
        let client = test_client("https://crates.io");
        let url = format!(
            "{}/api/v1/crates/{}/{}",
            client.base_url(),
            "my-crate",
            "1.2.3"
        );
        insta::assert_snapshot!("url_version", url);
    }

    #[test]
    fn snapshot_url_construction_owners() {
        let client = test_client("https://crates.io");
        let url = format!("{}/api/v1/crates/{}/owners", client.base_url(), "my-crate");
        insta::assert_snapshot!("url_owners", url);
    }

    #[test]
    fn snapshot_url_construction_custom_registry() {
        let client = test_client("https://127.0.0.1/");
        let url = format!("{}/api/v1/crates/{}", client.base_url(), "private-lib");
        insta::assert_snapshot!("url_custom_registry", url);
    }

    #[test]
    fn snapshot_sparse_index_paths() {
        insta::assert_snapshot!("sparse_path_1char", sparse_index_path("a"));
        insta::assert_snapshot!("sparse_path_2char", sparse_index_path("ab"));
        insta::assert_snapshot!("sparse_path_3char", sparse_index_path("abc"));
        insta::assert_snapshot!("sparse_path_4char", sparse_index_path("demo"));
        insta::assert_snapshot!("sparse_path_long", sparse_index_path("serde_json"));
    }

    #[test]
    fn snapshot_error_connection_refused() {
        let client = test_client("http://127.0.0.1:1");
        let err = client.crate_exists("anything").unwrap_err();
        insta::assert_snapshot!("error_connection_refused", err.to_string());
    }

    #[test]
    fn snapshot_error_unexpected_status_crate_exists() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 500, "");
        });
        let client = test_client(&base);
        let err = client.crate_exists("bad").unwrap_err();
        insta::assert_snapshot!("error_unexpected_status", err.to_string());
        handle.join().expect("join");
    }

    #[test]
    fn snapshot_error_owners_forbidden() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 403, "");
        });
        let client = test_client(&base);
        let err = client.list_owners("demo", "bad-token").unwrap_err();
        insta::assert_snapshot!("error_owners_forbidden", err.to_string());
        handle.join().expect("join");
    }

    #[test]
    fn snapshot_error_owners_not_found() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        let err = client.list_owners("nope", "token").unwrap_err();
        insta::assert_snapshot!("error_owners_not_found", err.to_string());
        handle.join().expect("join");
    }

    // ── property-based tests ─────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for valid crate name characters (alphanumeric, hyphen, underscore).
        fn crate_name_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z][a-zA-Z0-9_-]{0,63}".prop_filter("non-empty", |s| !s.is_empty())
        }

        /// Strategy for semver-like version strings.
        fn version_strategy() -> impl Strategy<Value = String> {
            (
                0u32..100,
                0u32..100,
                0u32..100,
                proptest::option::of("[a-z]{1,8}"),
            )
                .prop_map(|(major, minor, patch, pre)| match pre {
                    Some(tag) => format!("{major}.{minor}.{patch}-{tag}"),
                    None => format!("{major}.{minor}.{patch}"),
                })
        }

        proptest! {
            #[test]
            fn url_normalization_strips_trailing_slashes(
                base in Just("https://127.0.0.1".to_string()),
                slashes in "/{0,10}",
            ) {
                let input = format!("{base}{slashes}");
                let client = test_client(&input);
                let url = client.base_url();
                prop_assert!(!url.ends_with('/'), "URL still has trailing slash: {url}");
            }

            #[test]
            fn sparse_index_path_is_deterministic(name in crate_name_strategy()) {
                let a = sparse_index_path(&name);
                let b = sparse_index_path(&name);
                prop_assert_eq!(&a, &b, "sparse_index_path not deterministic for {}", name);
            }

            #[test]
            fn sparse_index_path_is_lowercase(name in crate_name_strategy()) {
                let path = sparse_index_path(&name);
                let path_lower = path.to_ascii_lowercase();
                prop_assert_eq!(path, path_lower,
                    "sparse_index_path should be all lowercase for {}", name);
            }

            #[test]
            fn crate_info_roundtrip_prop(
                name in "[a-z_-]{1,30}",
                version in version_strategy(),
                created in "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z",
                updated in "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z",
            ) {
                let info = CrateInfo {
                    name: name.clone(),
                    newest_version: version.clone(),
                    created_at: created.clone(),
                    updated_at: updated.clone(),
                };
                let json = serde_json::to_string(&info).expect("serialize");
                let back: CrateInfo = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(&back.name, &name);
                prop_assert_eq!(&back.newest_version, &version);
                prop_assert_eq!(&back.created_at, &created);
                prop_assert_eq!(&back.updated_at, &updated);
            }

            #[test]
            fn version_string_in_url_construction(
                version in version_strategy(),
            ) {
                let client = test_client("https://example.com");
                let expected = format!("https://example.com/api/v1/crates/test-crate/{version}");
                let url = format!("{}/api/v1/crates/{}/{}", client.base_url(), "test-crate", version);
                prop_assert_eq!(url, expected);
            }

            #[test]
            fn owners_response_roundtrip_prop(
                logins in prop::collection::vec("[a-z]{1,20}", 0..5),
            ) {
                let resp = OwnersResponse {
                    users: logins.iter().map(|login| OwnersApiUser {
                        id: None,
                        login: login.clone(),
                        name: None,
                        avatar: None,
                    }).collect(),
                };
                let json = serde_json::to_string(&resp).expect("serialize");
                let back: OwnersResponse = serde_json::from_str(&json).expect("deserialize");
                prop_assert_eq!(back.users.len(), resp.users.len());
                for (a, b) in resp.users.iter().zip(back.users.iter()) {
                    prop_assert_eq!(&a.login, &b.login);
                }
            }
        }
    }

    // ── Coverage gap fillers ─────────────────────────────────────────

    /// `get_owners` (no-token variant) when the registry returns 403:
    /// because `fetch_owners_with_token` maps 403/401 to the
    /// "forbidden" Err arm, `get_owners` propagates that error.
    /// Exercises the FORBIDDEN/UNAUTHORIZED match arm via the
    /// no-token entry point.
    #[test]
    fn get_owners_returns_error_on_403() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 403, "");
        });
        let client = test_client(&base);
        let err = client.get_owners("demo").unwrap_err();
        assert!(err.to_string().contains("forbidden"));
        handle.join().expect("join");
    }

    /// `is_owner` propagates the underlying `get_owners` error when the
    /// registry returns 403. Exercises the error-propagation path in
    /// `is_owner`.
    #[test]
    fn is_owner_propagates_owners_endpoint_error() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 403, "");
        });
        let client = test_client(&base);
        let err = client.is_owner("demo", "alice").unwrap_err();
        assert!(err.to_string().contains("forbidden"));
        handle.join().expect("join");
    }

    /// `is_version_visible_in_sparse_index` propagates fetch errors from
    /// `fetch_sparse_index_file` (e.g. 404 → "index file not found").
    /// Exercises the `?` propagation in `is_version_visible_in_sparse_index`.
    #[test]
    fn is_version_visible_in_sparse_index_propagates_404_error() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 404, "");
        });
        let client = test_client(&base);
        let err = client
            .is_version_visible_in_sparse_index(&base, "demo", "1.0.0")
            .unwrap_err();
        assert!(err.to_string().contains("index file not found"));
        handle.join().expect("join");
    }

    /// `version_exists` against an unexpected 401 status. Exercises the
    /// catch-all match arm with a 4xx that is neither 200 nor 404.
    #[test]
    fn version_exists_returns_error_on_401() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 401, "");
        });
        let client = test_client(&base);
        let err = client.version_exists("x", "0.1.0").unwrap_err();
        assert!(err.to_string().contains("unexpected status code"));
        handle.join().expect("join");
    }

    /// `crate_exists` against an unexpected 401 status. Exercises the
    /// catch-all match arm for a 4xx other than 404.
    #[test]
    fn crate_exists_returns_error_on_401() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 401, "");
        });
        let client = test_client(&base);
        let err = client.crate_exists("x").unwrap_err();
        assert!(err.to_string().contains("unexpected status code"));
        handle.join().expect("join");
    }

    /// `get_crate_info` against an unexpected 429 status — exercises the
    /// `!status.is_success()` error arm with a 4xx code that is not 404.
    #[test]
    fn get_crate_info_returns_error_on_429() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 429, "");
        });
        let client = test_client(&base);
        let err = client.get_crate_info("x").unwrap_err();
        assert!(err.to_string().contains("unexpected status code"));
        handle.join().expect("join");
    }

    /// `fetch_sparse_index_file` writes the cache content but does NOT
    /// create an etag file when the response has no ETag header. The next
    /// fetch should still produce content (server responds again), without
    /// sending an If-None-Match header.
    #[test]
    fn fetch_sparse_index_no_etag_does_not_create_etag_file() {
        let (server, base) = mock_server();
        let td = tempfile::tempdir().expect("tempdir");
        let cache_dir = td.path().to_path_buf();

        let handle = std::thread::spawn(move || {
            // No ETag header on response.
            respond(server.recv().expect("req"), 200, "{\"vers\":\"0.1.0\"}");
        });

        let client = test_client(&base).with_cache_dir(cache_dir.clone());
        let content = client
            .fetch_sparse_index_file(&base, "demo")
            .expect("fetch");
        assert!(content.contains("0.1.0"));

        // Cache file should exist; etag file should not.
        let cache_path = cache_dir.join("de").join("mo").join("demo");
        assert!(cache_path.exists(), "cache file should be written");

        let etag_path = cache_path.with_extension("etag");
        assert!(
            !etag_path.exists(),
            "etag file should NOT exist without an ETag header"
        );
        handle.join().expect("join");
    }

    /// `with_timeout` reconstructs the inner client and stores the new
    /// timeout. Verifies that chaining `with_timeout` followed by an
    /// actual request still works against a mock server.
    #[test]
    fn with_timeout_chained_client_can_make_request() {
        let (server, base) = mock_server();
        let handle = std::thread::spawn(move || {
            respond(server.recv().expect("req"), 200, "{}");
        });
        let client = test_client(&base).with_timeout(Duration::from_secs(5));
        assert_eq!(client.timeout, Duration::from_secs(5));
        assert!(client.crate_exists("any").expect("ok"));
        handle.join().expect("join");
    }
}
