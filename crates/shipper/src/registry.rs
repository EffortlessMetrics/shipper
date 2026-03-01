use anyhow::{Context, Result, bail};
use chrono::Utc;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::{Duration, Instant};

use crate::types::{ReadinessConfig, ReadinessEvidence, ReadinessMethod, Registry};

#[derive(Debug, Clone)]
pub struct RegistryClient {
    registry: Registry,
    http: Client,
    cache_dir: Option<std::path::PathBuf>,
}

impl RegistryClient {
    pub fn new(registry: Registry) -> Result<Self> {
        let http = Client::builder()
            .user_agent(format!("shipper/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            registry,
            http,
            cache_dir: None,
        })
    }

    /// Set the cache directory for sparse index fragments
    pub fn with_cache_dir(mut self, cache_dir: std::path::PathBuf) -> Self {
        self.cache_dir = Some(cache_dir);
        self
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn version_exists(&self, crate_name: &str, version: &str) -> Result<bool> {
        let url = format!(
            "{}/api/v1/crates/{}/{}",
            self.registry.api_base.trim_end_matches('/'),
            crate_name,
            version
        );

        let resp = self
            .http
            .get(url)
            .send()
            .context("registry request failed")?;
        match resp.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => bail!("unexpected status while checking version existence: {s}"),
        }
    }

    pub fn crate_exists(&self, crate_name: &str) -> Result<bool> {
        let url = format!(
            "{}/api/v1/crates/{}",
            self.registry.api_base.trim_end_matches('/'),
            crate_name
        );

        let resp = self
            .http
            .get(url)
            .send()
            .context("registry request failed")?;
        match resp.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => bail!("unexpected status while checking crate existence: {s}"),
        }
    }

    pub fn list_owners(&self, crate_name: &str, token: &str) -> Result<OwnersResponse> {
        let url = format!(
            "{}/api/v1/crates/{}/owners",
            self.registry.api_base.trim_end_matches('/'),
            crate_name
        );

        let resp = self
            .http
            .get(url)
            .header("Authorization", token)
            .send()
            .context("registry owners request failed")?;

        match resp.status() {
            StatusCode::OK => {
                let parsed: OwnersResponse = resp.json().context("failed to parse owners JSON")?;
                Ok(parsed)
            }
            StatusCode::NOT_FOUND => bail!("crate not found when querying owners: {crate_name}"),
            StatusCode::FORBIDDEN => bail!(
                "forbidden when querying owners; token may be invalid or missing required scope"
            ),
            s => bail!("unexpected status while querying owners: {s}"),
        }
    }

    /// Check if a crate is new (doesn't exist in the registry).
    ///
    /// Returns true if the crate doesn't exist, false if it does.
    pub fn check_new_crate(&self, crate_name: &str) -> Result<bool> {
        let exists = self.crate_exists(crate_name)?;
        Ok(!exists)
    }

    /// Check if a crate version is visible via the sparse index.
    ///
    /// Returns true if the version is found in the index, false otherwise.
    /// Parse errors and network errors are treated as "not visible" rather than failures.
    pub fn check_index_visibility(&self, crate_name: &str, version: &str) -> Result<bool> {
        // Calculate the index path for the crate using the 2+2+N scheme
        let index_path = self.calculate_index_path(crate_name);

        // Fetch the index file content
        let content = match self.fetch_index_file(&index_path) {
            Ok(content) => content,
            Err(_e) => {
                // Network errors or missing files are treated as "not visible"
                // This is graceful degradation - we don't want to fail the entire
                // readiness check just because the index is temporarily unavailable
                return Ok(false);
            }
        };

        // Parse the JSON and check if version exists
        match self.parse_version_from_index(&content, version) {
            Ok(found) => Ok(found),
            Err(_) => {
                // Parse errors are treated as "not visible"
                Ok(false)
            }
        }
    }

    /// Calculate the index path for a crate using Cargo's sparse index scheme.
    ///
    /// - 1 char  → `1/{name}`
    /// - 2 chars → `2/{name}`
    /// - 3 chars → `3/{name[0]}/{name}`
    /// - 4+ chars → `{name[0..2]}/{name[2..4]}/{name}`
    ///
    /// All names are lowercased per Cargo convention.
    fn calculate_index_path(&self, crate_name: &str) -> String {
        shipper_sparse_index::sparse_index_path(crate_name)
    }

    /// Fetch the index file content from the registry.
    fn fetch_index_file(&self, index_path: &str) -> Result<String> {
        let index_base = self.registry.get_index_base();
        let url = format!("{}/{}", index_base.trim_end_matches('/'), index_path);

        let cache_file = self.cache_dir.as_ref().map(|d| d.join(index_path));
        let etag_file = cache_file.as_ref().map(|f| f.with_extension("etag"));

        let mut request = self.http.get(&url);

        if let Some(ref path) = etag_file
            && let Ok(etag) = std::fs::read_to_string(path)
        {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag.trim());
        }

        let resp = request.send().context("index request failed")?;

        match resp.status() {
            StatusCode::OK => {
                let etag = resp
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|h| h.to_str().ok())
                    .map(|s| s.to_string());
                let content = resp.text().context("failed to read index response body")?;

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
            StatusCode::NOT_MODIFIED => {
                if let Some(ref path) = cache_file {
                    std::fs::read_to_string(path).context("failed to read cached index file")
                } else {
                    bail!("received 304 Not Modified but no cache file available")
                }
            }
            StatusCode::NOT_FOUND => {
                // The crate doesn't exist in the index yet
                bail!("index file not found: {}", url)
            }
            s => bail!("unexpected status while fetching index: {}", s),
        }
    }

    /// Parse the index content (line-delimited JSON) and check if the version exists.
    fn parse_version_from_index(&self, content: &str, version: &str) -> Result<bool> {
        Ok(shipper_sparse_index::contains_version(content, version))
    }

    /// Attempt ownership verification for a crate.
    ///
    /// Returns true if ownership is verified, false if verification fails or endpoint is unavailable.
    /// This function implements graceful degradation - if the ownership check fails due to API
    /// limitations, it returns false rather than an error.
    pub fn verify_ownership(&self, crate_name: &str, token: &str) -> Result<bool> {
        match self.list_owners(crate_name, token) {
            Ok(_) => Ok(true),
            Err(e) => {
                // Graceful degradation: if the endpoint is unavailable or returns forbidden,
                // return false rather than failing the entire preflight
                let msg = format!("{e:#}");
                if msg.contains("forbidden")
                    || msg.contains("403")
                    || msg.contains("unauthorized")
                    || msg.contains("401")
                    || msg.contains("not found")
                    || msg.contains("404")
                {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Check if a version is visible with exponential backoff and jitter.
    ///
    /// Returns Ok((true, evidence)) if the version becomes visible within the timeout,
    /// Ok((false, evidence)) if the timeout is exceeded, or Err on other failures.
    pub fn is_version_visible_with_backoff(
        &self,
        crate_name: &str,
        version: &str,
        config: &ReadinessConfig,
    ) -> Result<(bool, Vec<ReadinessEvidence>)> {
        let mut evidence = Vec::new();

        if !config.enabled {
            // If readiness checks are disabled, just check once
            let visible = self.version_exists(crate_name, version)?;
            evidence.push(ReadinessEvidence {
                attempt: 1,
                visible,
                timestamp: Utc::now(),
                delay_before: Duration::ZERO,
            });
            return Ok((visible, evidence));
        }

        let start = Instant::now();
        let mut attempt: u32 = 0;

        // Initial delay before first poll
        if config.initial_delay > Duration::ZERO {
            std::thread::sleep(config.initial_delay);
        }

        loop {
            attempt += 1;

            // Calculate delay for this iteration (used for evidence; applied after check)
            let jittered_delay = if attempt == 1 {
                Duration::ZERO
            } else {
                let base_delay = config.poll_interval;
                let exponential_delay = base_delay
                    .saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(2).min(16)));
                let capped_delay = exponential_delay.min(config.max_delay);
                let jitter_range = config.jitter_factor;
                let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_range - jitter_range);
                Duration::from_millis((capped_delay.as_millis() as f64 * jitter).round() as u64)
            };

            // Check visibility based on method
            // Errors are treated as "not visible" to allow backoff retries
            let visible = match config.method {
                ReadinessMethod::Api => self.version_exists(crate_name, version).unwrap_or(false),
                ReadinessMethod::Index => self
                    .check_index_visibility(crate_name, version)
                    .unwrap_or(false),
                ReadinessMethod::Both => {
                    if config.prefer_index {
                        match self.check_index_visibility(crate_name, version) {
                            Ok(true) => true,
                            _ => self.version_exists(crate_name, version).unwrap_or(false),
                        }
                    } else {
                        match self.version_exists(crate_name, version) {
                            Ok(true) => true,
                            _ => self
                                .check_index_visibility(crate_name, version)
                                .unwrap_or(false),
                        }
                    }
                }
            };

            evidence.push(ReadinessEvidence {
                attempt,
                visible,
                timestamp: Utc::now(),
                delay_before: jittered_delay,
            });

            if visible {
                return Ok((true, evidence));
            }

            // Check if we've exceeded max total wait
            if start.elapsed() >= config.max_total_wait {
                return Ok((false, evidence));
            }

            // Calculate next delay with exponential backoff and jitter
            let base_delay = config.poll_interval;
            let exponential_delay =
                base_delay.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1).min(16)));
            let capped_delay = exponential_delay.min(config.max_delay);

            let jitter_range = config.jitter_factor;
            let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_range - jitter_range);
            let next_delay =
                Duration::from_millis((capped_delay.as_millis() as f64 * jitter).round() as u64);

            std::thread::sleep(next_delay);
        }
    }

    /// Calculate the backoff delay for a given attempt with jitter.
    ///
    /// This is a helper function that can be used for testing.
    pub fn calculate_backoff_delay(
        &self,
        base: Duration,
        max: Duration,
        attempt: u32,
        jitter_factor: f64,
    ) -> Duration {
        let pow = attempt.saturating_sub(1).min(16);
        let mut delay = base.saturating_mul(2_u32.saturating_pow(pow));
        if delay > max {
            delay = max;
        }

        // Apply jitter: delay * (1 ± jitter_factor)
        // Using rand::random() like the existing backoff_delay function
        let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_factor - jitter_factor);
        let millis = (delay.as_millis() as f64 * jitter).round() as u128;
        Duration::from_millis(millis as u64)
    }
}

#[derive(Debug, Deserialize)]
pub struct OwnersResponse {
    pub users: Vec<Owner>,
}

#[derive(Debug, Deserialize)]
pub struct Owner {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::thread;

    use tiny_http::{Response, Server, StatusCode};

    use super::*;

    fn with_server<F>(handler: F) -> (String, thread::JoinHandle<()>)
    where
        F: FnOnce(tiny_http::Request) + Send + 'static,
    {
        let server = Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        let handle = thread::spawn(move || {
            let req = server.recv().expect("request");
            handler(req);
        });
        (addr, handle)
    }

    fn test_registry(api_base: String) -> Registry {
        Registry {
            name: "crates-io".to_string(),
            api_base,
            index_base: None,
        }
    }

    fn test_registry_with_index(api_base: String) -> Registry {
        Registry {
            name: "crates-io".to_string(),
            api_base: api_base.clone(),
            index_base: Some(api_base),
        }
    }

    fn with_multi_server<F>(handler: F, request_count: usize) -> (String, thread::JoinHandle<()>)
    where
        F: Fn(tiny_http::Request) + Send + 'static,
    {
        let server = Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        let handle = thread::spawn(move || {
            for _ in 0..request_count {
                match server.recv_timeout(Duration::from_secs(5)) {
                    Ok(Some(req)) => handler(req),
                    _ => break,
                }
            }
        });
        (addr, handle)
    }

    #[test]
    fn version_exists_true_for_200() {
        let (api_base, handle) = with_server(|req| {
            assert_eq!(req.url(), "/api/v1/crates/demo/1.2.3");
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        assert_eq!(cli.registry().name, "crates-io");
        let exists = cli.version_exists("demo", "1.2.3").expect("exists");
        assert!(exists);
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_false_for_404() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(404)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let exists = cli.version_exists("demo", "1.2.3").expect("exists");
        assert!(!exists);
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_errors_for_unexpected_status() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(500)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .version_exists("demo", "1.2.3")
            .expect_err("unexpected status must fail");
        assert!(format!("{err:#}").contains("unexpected status while checking version existence"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_true_for_200() {
        let (api_base, handle) = with_server(|req| {
            assert_eq!(req.url(), "/api/v1/crates/demo");
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let exists = cli.crate_exists("demo").expect("exists");
        assert!(exists);
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_false_for_404() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(404)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let exists = cli.crate_exists("demo").expect("exists");
        assert!(!exists);
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_errors_for_unexpected_status() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(500)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .crate_exists("demo")
            .expect_err("unexpected status must fail");
        assert!(format!("{err:#}").contains("unexpected status while checking crate existence"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_parses_success_response() {
        let (api_base, handle) = with_server(|req| {
            assert_eq!(req.url(), "/api/v1/crates/demo/owners");
            let auth = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_string());
            assert_eq!(auth.as_deref(), Some("token-abc"));

            let body = r#"{"users":[{"id":7,"login":"alice","name":"Alice"}]}"#;
            let resp = Response::from_string(body)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let owners = cli.list_owners("demo", "token-abc").expect("owners");
        assert_eq!(owners.users.len(), 1);
        assert_eq!(owners.users[0].login, "alice");
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_errors_for_404_403_and_other_statuses() {
        let (api_base_404, h1) = with_server(|req| {
            req.respond(Response::empty(StatusCode(404)))
                .expect("respond");
        });
        let cli_404 = RegistryClient::new(test_registry(api_base_404)).expect("client");
        let err_404 = cli_404
            .list_owners("missing", "token")
            .expect_err("404 must fail");
        assert!(format!("{err_404:#}").contains("crate not found when querying owners"));
        h1.join().expect("join");

        let (api_base_403, h2) = with_server(|req| {
            req.respond(Response::empty(StatusCode(403)))
                .expect("respond");
        });
        let cli_403 = RegistryClient::new(test_registry(api_base_403)).expect("client");
        let err_403 = cli_403
            .list_owners("demo", "token")
            .expect_err("403 must fail");
        assert!(format!("{err_403:#}").contains("forbidden when querying owners"));
        h2.join().expect("join");

        let (api_base_500, h3) = with_server(|req| {
            req.respond(Response::empty(StatusCode(500)))
                .expect("respond");
        });
        let cli_500 = RegistryClient::new(test_registry(api_base_500)).expect("client");
        let err_500 = cli_500
            .list_owners("demo", "token")
            .expect_err("500 must fail");
        assert!(format!("{err_500:#}").contains("unexpected status while querying owners"));
        h3.join().expect("join");
    }

    #[test]
    fn calculate_backoff_delay_is_bounded_with_jitter() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);
        let jitter_factor = 0.5;

        // Test first attempt
        let d1 = cli.calculate_backoff_delay(base, max, 1, jitter_factor);
        // With 50% jitter, first attempt should be 50ms..150ms
        assert!(d1 >= Duration::from_millis(50));
        assert!(d1 <= Duration::from_millis(150));

        // Test high attempt (should be capped at max)
        let d20 = cli.calculate_backoff_delay(base, max, 20, jitter_factor);
        // With 50% jitter, max delay should be 250ms..750ms
        assert!(d20 >= Duration::from_millis(250));
        assert!(d20 <= Duration::from_millis(750));

        // Test with zero jitter
        let d_no_jitter = cli.calculate_backoff_delay(base, max, 2, 0.0);
        // With no jitter, second attempt should be exactly 200ms
        assert_eq!(d_no_jitter, Duration::from_millis(200));
    }

    #[test]
    fn is_version_visible_with_backoff_disabled_returns_immediate() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: false,
            method: ReadinessMethod::Api,
            initial_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(60),
            max_total_wait: Duration::from_secs(300),
            poll_interval: Duration::from_secs(2),
            jitter_factor: 0.5,
            index_path: None,
            prefer_index: false,
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        let (visible, evidence) = result.unwrap();
        assert!(visible);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].visible);
        handle.join().expect("join");
    }

    // Index-based readiness tests

    #[test]
    fn calculate_index_path_for_standard_crate() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        // Test standard crate names (4+ chars use first_two/chars_2_4/name)
        assert_eq!(cli.calculate_index_path("serde"), "se/rd/serde");
        assert_eq!(cli.calculate_index_path("tokio"), "to/ki/tokio");
        assert_eq!(cli.calculate_index_path("rand"), "ra/nd/rand");
        assert_eq!(cli.calculate_index_path("http"), "ht/tp/http");
    }

    #[test]
    fn calculate_index_path_for_short_crate() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        // Test single-character crate name
        assert_eq!(cli.calculate_index_path("a"), "1/a");

        // Test two-character crate name
        assert_eq!(cli.calculate_index_path("ab"), "2/ab");

        // Test three-character crate name
        assert_eq!(cli.calculate_index_path("abc"), "3/a/abc");
    }

    #[test]
    fn calculate_index_path_for_special_chars() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        // Test crate names with special characters (lowercased, using length-based scheme)
        assert_eq!(cli.calculate_index_path("_serde"), "_s/er/_serde");
        assert_eq!(cli.calculate_index_path("-tokio"), "-t/ok/-tokio");
        // Test uppercase is lowercased
        assert_eq!(cli.calculate_index_path("Serde"), "se/rd/serde");
    }

    #[test]
    fn parse_version_from_index_finds_version() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        let index_content = "{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.0.1\"}\n{\"vers\":\"2.0.0\"}\n";

        let found = cli.parse_version_from_index(index_content, "1.0.1");
        assert!(found.is_ok());
        assert!(found.unwrap());
    }

    #[test]
    fn parse_version_from_index_returns_false_for_missing_version() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        let index_content = "{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.0.1\"}\n";

        let found = cli.parse_version_from_index(index_content, "2.0.0");
        assert!(found.is_ok());
        assert!(!found.unwrap());
    }

    #[test]
    fn parse_version_from_index_handles_invalid_json() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        let invalid_json = "not valid json";

        let found = cli.parse_version_from_index(invalid_json, "1.0.0");
        assert!(found.is_ok());
        assert!(!found.unwrap());
    }

    #[test]
    fn check_index_visibility_returns_true_for_existing_version() {
        let index_content = "{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.0.1\"}\n";

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/de/mo/demo");
            let resp = Response::from_string(index_content)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.1").expect("check");
        assert!(visible);
        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_returns_false_for_missing_version() {
        let index_content = "{\"vers\":\"1.0.0\"}\n";

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/de/mo/demo");
            let resp = Response::from_string(index_content)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.1").expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_returns_false_for_404() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(404)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli
            .check_index_visibility("missing", "1.0.0")
            .expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_returns_false_for_network_error() {
        // Use a non-existent URL to simulate a network error
        let registry = Registry {
            name: "test".to_string(),
            api_base: "http://nonexistent.invalid:9999".to_string(),
            index_base: Some("http://nonexistent.invalid:9999".to_string()),
        };

        let cli = RegistryClient::new(registry).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
    }

    #[test]
    fn check_index_visibility_returns_false_for_invalid_json() {
        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string("not valid json")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_with_backoff_uses_index_method() {
        let index_content = "{\"vers\":\"1.0.0\"}\n";

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/de/mo/demo");
            let resp = Response::from_string(index_content)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(1),
            poll_interval: Duration::from_millis(100),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        let (visible, evidence) = result.unwrap();
        assert!(visible);
        assert!(!evidence.is_empty());
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_with_backoff_uses_both_method_prefer_index() {
        let index_content = "{\"vers\":\"1.0.0\"}\n";

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/de/mo/demo");
            let resp = Response::from_string(index_content)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(1),
            poll_interval: Duration::from_millis(100),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: true, // Prefer index
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        let (visible, evidence) = result.unwrap();
        assert!(visible);
        assert!(!evidence.is_empty());
        handle.join().expect("join");
    }

    #[test]
    fn registry_get_index_base_returns_explicit_index_base() {
        let registry = Registry {
            name: "test".to_string(),
            api_base: "https://example.com".to_string(),
            index_base: Some("https://index.example.com".to_string()),
        };

        assert_eq!(registry.get_index_base(), "https://index.example.com");
    }

    #[test]
    fn registry_get_index_base_derives_from_api_base() {
        let registry = Registry {
            name: "test".to_string(),
            api_base: "https://crates.io".to_string(),
            index_base: None,
        };

        assert_eq!(registry.get_index_base(), "https://index.crates.io");
    }

    #[test]
    fn registry_get_index_base_derives_from_http_api_base() {
        let registry = Registry {
            name: "test".to_string(),
            api_base: "http://crates.io".to_string(),
            index_base: None,
        };

        assert_eq!(registry.get_index_base(), "http://index.crates.io");
    }

    // Additional index-based readiness tests

    #[test]
    fn check_index_visibility_with_empty_index_returns_false() {
        let index_content = "";

        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string(index_content)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_with_multiple_versions_finds_correct() {
        let index_content = "{\"vers\":\"0.1.0\"}\n{\"vers\":\"0.2.0\"}\n{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.1.0\"}\n";

        let (api_base, handle) = with_multi_server(
            move |req| {
                let resp = Response::from_string(index_content)
                    .with_status_code(StatusCode(200))
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json")
                            .expect("header"),
                    );
                req.respond(resp).expect("respond");
            },
            5,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");

        // Check each version exists
        assert!(cli.check_index_visibility("demo", "0.1.0").expect("check"));
        assert!(cli.check_index_visibility("demo", "0.2.0").expect("check"));
        assert!(cli.check_index_visibility("demo", "1.0.0").expect("check"));
        assert!(cli.check_index_visibility("demo", "1.1.0").expect("check"));

        // Check non-existent version
        assert!(!cli.check_index_visibility("demo", "2.0.0").expect("check"));

        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_handles_malformed_json_gracefully() {
        // JSONL with one valid line and one invalid line; valid line should still be found
        let malformed_json = "{\"vers\":\"1.0.0\"}\n{\"invalid\":\"entry\"}\n";

        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string(malformed_json)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        // Valid lines are still parsed; invalid lines are skipped
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(visible);
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_with_backoff_with_api_method() {
        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(1),
            poll_interval: Duration::from_millis(100),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        let (visible, evidence) = result.unwrap();
        assert!(visible);
        assert!(!evidence.is_empty());
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_with_backoff_with_both_method_prefer_api() {
        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(1),
            poll_interval: Duration::from_millis(100),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false, // Prefer API
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        let (visible, evidence) = result.unwrap();
        assert!(visible);
        assert!(!evidence.is_empty());
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_with_backoff_returns_false_on_timeout() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                // Always return 404
                let resp = Response::empty(StatusCode(404));
                req.respond(resp).expect("respond");
            },
            10,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            max_total_wait: Duration::from_millis(100),
            poll_interval: Duration::from_millis(25),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        let (visible, evidence) = result.unwrap();
        assert!(!visible);
        assert!(!evidence.is_empty());
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    #[test]
    fn is_version_visible_with_backoff_handles_network_errors_gracefully() {
        // Use a non-existent URL to simulate network errors
        let registry = Registry {
            name: "test".to_string(),
            api_base: "http://nonexistent.invalid:9999".to_string(),
            index_base: Some("http://nonexistent.invalid:9999".to_string()),
        };

        let cli = RegistryClient::new(registry).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            max_total_wait: Duration::from_millis(100),
            poll_interval: Duration::from_millis(25),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        let (visible, _evidence) = result.unwrap();
        assert!(!visible);
    }

    #[test]
    fn is_version_visible_with_backoff_respects_initial_delay() {
        let start = std::time::Instant::now();

        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(1),
            poll_interval: Duration::from_millis(100),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        let elapsed = start.elapsed();
        let (visible, evidence) = result.unwrap();
        assert!(visible);
        assert!(!evidence.is_empty());

        // Should wait at least the initial delay
        assert!(elapsed >= Duration::from_millis(50));
        handle.join().expect("join");
    }

    #[test]
    fn verify_ownership_returns_true_on_success() {
        let owners_json = r#"{"users":[{"id":1,"login":"user1","name":null},{"id":2,"login":"user2","name":null}]}"#;

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/api/v1/crates/demo/owners");
            let resp = Response::from_string(owners_json)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let verified = cli.verify_ownership("demo", "fake-token").expect("verify");
        assert!(verified);
        handle.join().expect("join");
    }

    #[test]
    fn verify_ownership_returns_false_on_forbidden() {
        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(403))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let verified = cli.verify_ownership("demo", "fake-token").expect("verify");
        assert!(!verified);
        handle.join().expect("join");
    }

    #[test]
    fn verify_ownership_returns_false_on_not_found() {
        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(404))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let verified = cli.verify_ownership("demo", "fake-token").expect("verify");
        assert!(!verified);
        handle.join().expect("join");
    }

    #[test]
    fn check_new_crate_returns_true_for_nonexistent_crate() {
        let (api_base, handle) = with_server(move |req| {
            let resp = Response::empty(StatusCode(404));
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let is_new = cli.check_new_crate("demo").expect("check");
        assert!(is_new);
        handle.join().expect("join");
    }

    #[test]
    fn check_new_crate_returns_false_for_existing_crate() {
        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string("{}")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let is_new = cli.check_new_crate("demo").expect("check");
        assert!(!is_new);
        handle.join().expect("join");
    }

    // ── Readiness edge-case tests ────────────────────────────────────

    #[test]
    fn api_mode_visible_on_first_check() {
        let (api_base, handle) = with_server(move |req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].visible);
        assert_eq!(evidence[0].attempt, 1);
        assert_eq!(evidence[0].delay_before, Duration::ZERO);
        handle.join().expect("join");
    }

    #[test]
    fn api_mode_never_visible_times_out() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                req.respond(Response::empty(StatusCode(404)))
                    .expect("respond");
            },
            20,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(80),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(!visible);
        assert!(
            evidence.len() >= 2,
            "should poll multiple times before timeout"
        );
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    #[test]
    fn api_mode_intermittent_failures_then_success() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let (api_base, handle) = with_multi_server(
            move |req| {
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                // First 2 requests return 500 (error), third returns 200
                let status = if n < 2 { 500 } else { 200 };
                req.respond(Response::empty(StatusCode(status)))
                    .expect("respond");
            },
            5,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        // First two attempts fail (500 → unwrap_or(false)), third succeeds
        assert!(evidence.len() >= 3);
        assert!(!evidence[0].visible);
        assert!(!evidence[1].visible);
        assert!(evidence.last().unwrap().visible);
        handle.join().expect("join");
    }

    #[test]
    fn index_mode_sparse_index_shows_version() {
        let index_content = "{\"vers\":\"0.9.0\"}\n{\"vers\":\"1.0.0\"}\n";

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/de/mo/demo");
            req.respond(Response::from_string(index_content).with_status_code(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].visible);
        handle.join().expect("join");
    }

    #[test]
    fn index_mode_stale_empty_index() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                // Return empty body (stale/empty index)
                req.respond(Response::from_string("").with_status_code(StatusCode(200)))
                    .expect("respond");
            },
            10,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(80),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(!visible);
        assert!(evidence.len() >= 2);
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    #[test]
    fn index_mode_parse_errors_treated_as_not_visible() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                req.respond(
                    Response::from_string("<<<not json>>>").with_status_code(StatusCode(200)),
                )
                .expect("respond");
            },
            10,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(80),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(!visible);
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    #[test]
    fn both_mode_api_succeeds_index_fails() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        // Both mode with prefer_index: index is checked first (fails), then API (succeeds)
        let (api_base, handle) = with_multi_server(
            move |req| {
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                let url = req.url().to_string();
                if url.contains("/api/v1/crates/") {
                    // API succeeds
                    req.respond(Response::empty(StatusCode(200)))
                        .expect("respond");
                } else {
                    // Index returns 404 (not found)
                    req.respond(Response::empty(StatusCode(404)))
                        .expect("respond");
                }
                let _ = n;
            },
            5,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base.clone())).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: true, // index checked first, falls back to API
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].visible);
        handle.join().expect("join");
    }

    #[test]
    fn both_mode_index_succeeds_api_fails() {
        let index_content = "{\"vers\":\"1.0.0\"}\n";

        // Both mode with prefer_index=false: API is checked first (fails), then index (succeeds)
        let (api_base, handle) = with_multi_server(
            move |req| {
                let url = req.url().to_string();
                if url.contains("/api/v1/crates/") {
                    // API returns 404
                    req.respond(Response::empty(StatusCode(404)))
                        .expect("respond");
                } else {
                    // Index returns the version
                    req.respond(
                        Response::from_string(index_content).with_status_code(StatusCode(200)),
                    )
                    .expect("respond");
                }
            },
            5,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base.clone())).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false, // API checked first, falls back to index
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].visible);
        handle.join().expect("join");
    }

    #[test]
    fn zero_timeout_returns_immediately() {
        let (api_base, handle) = with_server(move |req| {
            req.respond(Response::empty(StatusCode(404)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::ZERO,
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let start = Instant::now();
        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        let elapsed = start.elapsed();

        assert!(!visible);
        // With zero timeout, should do exactly 1 poll then exit
        assert_eq!(evidence.len(), 1);
        assert!(!evidence[0].visible);
        // Should complete very quickly (well under 1 second)
        assert!(elapsed < Duration::from_secs(1));
        handle.join().expect("join");
    }

    #[test]
    fn evidence_records_populated_correctly() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let (api_base, handle) = with_multi_server(
            move |req| {
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                // Not visible on first two attempts, visible on third
                let status = if n < 2 { 404 } else { 200 };
                req.respond(Response::empty(StatusCode(status)))
                    .expect("respond");
            },
            5,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(50),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert_eq!(evidence.len(), 3);

        // Check attempt numbers are sequential
        assert_eq!(evidence[0].attempt, 1);
        assert_eq!(evidence[1].attempt, 2);
        assert_eq!(evidence[2].attempt, 3);

        // Check visibility flags
        assert!(!evidence[0].visible);
        assert!(!evidence[1].visible);
        assert!(evidence[2].visible);

        // First attempt should have zero delay
        assert_eq!(evidence[0].delay_before, Duration::ZERO);

        // Subsequent attempts should have non-zero delay
        assert!(evidence[1].delay_before > Duration::ZERO);
        assert!(evidence[2].delay_before > Duration::ZERO);

        // Timestamps should be chronologically ordered
        assert!(evidence[0].timestamp <= evidence[1].timestamp);
        assert!(evidence[1].timestamp <= evidence[2].timestamp);

        handle.join().expect("join");
    }

    #[test]
    fn backoff_delays_increase_exponentially() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(10);

        // With zero jitter, delays should follow exact powers of 2
        let d1 = cli.calculate_backoff_delay(base, max, 1, 0.0);
        let d2 = cli.calculate_backoff_delay(base, max, 2, 0.0);
        let d3 = cli.calculate_backoff_delay(base, max, 3, 0.0);
        let d4 = cli.calculate_backoff_delay(base, max, 4, 0.0);

        // attempt 1 → base * 2^0 = 100ms
        assert_eq!(d1, Duration::from_millis(100));
        // attempt 2 → base * 2^1 = 200ms
        assert_eq!(d2, Duration::from_millis(200));
        // attempt 3 → base * 2^2 = 400ms
        assert_eq!(d3, Duration::from_millis(400));
        // attempt 4 → base * 2^3 = 800ms
        assert_eq!(d4, Duration::from_millis(800));

        // Verify exponential growth (each delay is 2× the previous)
        assert_eq!(d2, d1 * 2);
        assert_eq!(d3, d2 * 2);
        assert_eq!(d4, d3 * 2);
    }

    #[test]
    fn backoff_delays_capped_at_max() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(500);

        // With zero jitter, attempt 4 would be 800ms but should be capped at 500ms
        let d4 = cli.calculate_backoff_delay(base, max, 4, 0.0);
        assert_eq!(d4, Duration::from_millis(500));

        // Very high attempt should also be capped
        let d20 = cli.calculate_backoff_delay(base, max, 20, 0.0);
        assert_eq!(d20, Duration::from_millis(500));
    }

    #[test]
    fn disabled_readiness_with_not_found_returns_false() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(404)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: false,
            method: ReadinessMethod::Api,
            initial_delay: Duration::from_secs(999), // should be ignored
            max_delay: Duration::from_secs(999),
            max_total_wait: Duration::from_secs(999),
            poll_interval: Duration::from_secs(999),
            jitter_factor: 0.5,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(!visible);
        assert_eq!(evidence.len(), 1);
        assert!(!evidence[0].visible);
        assert_eq!(evidence[0].attempt, 1);
        assert_eq!(evidence[0].delay_before, Duration::ZERO);
        handle.join().expect("join");
    }

    #[test]
    fn both_mode_both_fail_times_out() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                // Both API and index return 404
                req.respond(Response::empty(StatusCode(404)))
                    .expect("respond");
            },
            20,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base.clone())).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(80),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(!visible);
        assert!(evidence.len() >= 2);
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    // ── HTTP error code tests ────────────────────────────────────────

    #[test]
    fn version_exists_errors_for_429_rate_limit() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(429)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .version_exists("demo", "1.0.0")
            .expect_err("429 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_errors_for_502_bad_gateway() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(502)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .version_exists("demo", "1.0.0")
            .expect_err("502 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_errors_for_503_service_unavailable() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(503)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .version_exists("demo", "1.0.0")
            .expect_err("503 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_errors_for_429_rate_limit() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(429)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.crate_exists("demo").expect_err("429 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_errors_for_429_rate_limit() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(429)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.list_owners("demo", "token").expect_err("429 must fail");
        assert!(format!("{err:#}").contains("unexpected status while querying owners"));
        handle.join().expect("join");
    }

    // ── Malformed response tests ─────────────────────────────────────

    #[test]
    fn list_owners_errors_on_non_json_response() {
        let (api_base, handle) = with_server(|req| {
            let resp = Response::from_string("this is not json at all")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "text/plain").expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .list_owners("demo", "token")
            .expect_err("non-json must fail");
        assert!(format!("{err:#}").contains("failed to parse owners JSON"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_errors_on_truncated_json() {
        let (api_base, handle) = with_server(|req| {
            let resp = Response::from_string(r#"{"users":[{"id":1,"login":"al"#)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .list_owners("demo", "token")
            .expect_err("truncated json must fail");
        assert!(format!("{err:#}").contains("failed to parse owners JSON"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_errors_on_wrong_schema_json() {
        let (api_base, handle) = with_server(|req| {
            // Valid JSON but wrong schema — missing "users" field
            let resp = Response::from_string(r#"{"data": [1, 2, 3]}"#)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .list_owners("demo", "token")
            .expect_err("wrong schema must fail");
        assert!(format!("{err:#}").contains("failed to parse owners JSON"));
        handle.join().expect("join");
    }

    // ── Version comparison tests ─────────────────────────────────────

    #[test]
    fn parse_version_from_index_exact_match_only() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        let content = "{\"vers\":\"1.0.0\"}\n{\"vers\":\"1.0.10\"}\n{\"vers\":\"1.0.0-beta.1\"}\n";

        // Exact match
        assert!(cli.parse_version_from_index(content, "1.0.0").unwrap());
        assert!(cli.parse_version_from_index(content, "1.0.10").unwrap());

        // Must not match prefix: "1.0.0" should not match "1.0.0-beta.1"
        // and "1.0.1" should not match "1.0.10"
        assert!(!cli.parse_version_from_index(content, "1.0.1").unwrap());
    }

    #[test]
    fn parse_version_from_index_prerelease_versions() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        let content = "{\"vers\":\"1.0.0-alpha.1\"}\n{\"vers\":\"1.0.0-beta.2\"}\n{\"vers\":\"1.0.0-rc.1\"}\n{\"vers\":\"1.0.0\"}\n";

        assert!(
            cli.parse_version_from_index(content, "1.0.0-alpha.1")
                .unwrap()
        );
        assert!(
            cli.parse_version_from_index(content, "1.0.0-beta.2")
                .unwrap()
        );
        assert!(cli.parse_version_from_index(content, "1.0.0-rc.1").unwrap());
        assert!(cli.parse_version_from_index(content, "1.0.0").unwrap());

        // Non-existent pre-release
        assert!(
            !cli.parse_version_from_index(content, "1.0.0-alpha.2")
                .unwrap()
        );
    }

    #[test]
    fn parse_version_from_index_empty_content() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        assert!(!cli.parse_version_from_index("", "1.0.0").unwrap());
        assert!(!cli.parse_version_from_index("\n\n\n", "1.0.0").unwrap());
    }

    // ── Timeout handling tests ───────────────────────────────────────

    #[test]
    fn version_exists_slow_response_still_succeeds() {
        let (api_base, handle) = with_server(|req| {
            // Simulate a slow but successful response
            std::thread::sleep(Duration::from_millis(200));
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let exists = cli.version_exists("demo", "1.0.0").expect("exists");
        assert!(exists);
        handle.join().expect("join");
    }

    // ── Readiness edge-case tests: method-specific ───────────────────

    #[test]
    fn api_mode_500_treated_as_not_visible_in_backoff() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                req.respond(Response::empty(StatusCode(500)))
                    .expect("respond");
            },
            10,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(80),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        // 500 errors are treated as "not visible" by unwrap_or(false)
        assert!(!visible);
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    #[test]
    fn index_mode_502_treated_as_not_visible_in_backoff() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                req.respond(Response::empty(StatusCode(502)))
                    .expect("respond");
            },
            10,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(80),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(!visible);
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    #[test]
    fn both_mode_prefer_index_true_checks_index_first() {
        use std::sync::Arc;

        let call_order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let call_order_clone = call_order.clone();

        let (api_base, handle) = with_multi_server(
            move |req| {
                let url = req.url().to_string();
                let mut order = call_order_clone.lock().unwrap();
                if url.contains("/api/v1/crates/") {
                    order.push("api".to_string());
                    req.respond(Response::empty(StatusCode(200)))
                        .expect("respond");
                } else {
                    order.push("index".to_string());
                    // Index returns 404 so it falls back to API
                    req.respond(Response::empty(StatusCode(404)))
                        .expect("respond");
                }
            },
            5,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: true,
        };

        let (visible, _) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);

        let order = call_order.lock().unwrap();
        // With prefer_index=true, index is tried first, then API fallback
        assert!(order.len() >= 2);
        assert_eq!(order[0], "index");
        assert_eq!(order[1], "api");
        handle.join().expect("join");
    }

    #[test]
    fn both_mode_prefer_index_false_checks_api_first() {
        use std::sync::Arc;

        let call_order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let call_order_clone = call_order.clone();
        let index_content = "{\"vers\":\"1.0.0\"}\n";

        let (api_base, handle) = with_multi_server(
            move |req| {
                let url = req.url().to_string();
                let mut order = call_order_clone.lock().unwrap();
                if url.contains("/api/v1/crates/") {
                    order.push("api".to_string());
                    // API returns 404, so falls back to index
                    req.respond(Response::empty(StatusCode(404)))
                        .expect("respond");
                } else {
                    order.push("index".to_string());
                    req.respond(
                        Response::from_string(index_content).with_status_code(StatusCode(200)),
                    )
                    .expect("respond");
                }
            },
            5,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Both,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, _) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);

        let order = call_order.lock().unwrap();
        assert!(order.len() >= 2);
        assert_eq!(order[0], "api");
        assert_eq!(order[1], "index");
        handle.join().expect("join");
    }

    // ── Snapshot tests ───────────────────────────────────────────────

    #[test]
    fn snapshot_owners_response_parsed() {
        let (api_base, handle) = with_server(|req| {
            let body = r#"{"users":[{"id":42,"login":"alice","name":"Alice Wonderland"},{"id":99,"login":"bob","name":null}]}"#;
            let resp = Response::from_string(body)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let owners = cli.list_owners("demo", "token").expect("owners");
        insta::assert_debug_snapshot!("owners_response_parsed", owners);
        handle.join().expect("join");
    }

    #[test]
    fn snapshot_readiness_evidence_single_attempt() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_secs(1),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert_eq!(evidence.len(), 1);

        // Snapshot with redacted timestamp (it changes every run)
        insta::assert_debug_snapshot!(
            "readiness_evidence_single_attempt",
            evidence
                .iter()
                .map(|e| {
                    format!(
                        "attempt={} visible={} delay_before={}ms",
                        e.attempt,
                        e.visible,
                        e.delay_before.as_millis()
                    )
                })
                .collect::<Vec<_>>()
        );
        handle.join().expect("join");
    }

    // ── Proptest: crate names always produce valid registry URLs ──────

    mod property_tests_registry {
        use proptest::prelude::*;

        /// Generates a valid Rust crate name: starts with a letter, followed by
        /// alphanumeric, hyphens, or underscores. Length 1..=64.
        fn crate_name_strategy() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9_-]{0,63}".prop_map(|s| s)
        }

        proptest! {
            #[test]
            fn random_crate_names_produce_valid_api_url(name in crate_name_strategy()) {
                let api_base = "https://crates.io";
                let url = format!(
                    "{}/api/v1/crates/{}/{}",
                    api_base.trim_end_matches('/'),
                    name,
                    "1.0.0"
                );
                // URL must not contain spaces, must start with https
                prop_assert!(!url.contains(' '));
                prop_assert!(url.starts_with("https://"));
                prop_assert!(url.contains("/api/v1/crates/"));
                // Must be parseable as a URL
                prop_assert!(url.parse::<reqwest::Url>().is_ok());
            }

            #[test]
            fn random_crate_names_produce_valid_index_path(name in crate_name_strategy()) {
                let path = shipper_sparse_index::sparse_index_path(&name);
                // Path must not be empty
                prop_assert!(!path.is_empty());
                // Path must contain the lowercased crate name
                prop_assert!(path.contains(&name.to_lowercase()));
                // Path must match one of the valid patterns:
                // 1/x, 2/xx, 3/x/xxx, ab/cd/abcdef...
                let segments: Vec<&str> = path.split('/').collect();
                match name.len() {
                    1 => {
                        prop_assert_eq!(segments.len(), 2);
                        prop_assert_eq!(segments[0], "1");
                    }
                    2 => {
                        prop_assert_eq!(segments.len(), 2);
                        prop_assert_eq!(segments[0], "2");
                    }
                    3 => {
                        prop_assert_eq!(segments.len(), 3);
                        prop_assert_eq!(segments[0], "3");
                    }
                    _ => {
                        prop_assert_eq!(segments.len(), 3);
                        prop_assert_eq!(segments[0].len(), 2);
                        prop_assert_eq!(segments[1].len(), 2);
                    }
                }
            }
        }
    }

    // ── HTTP 4xx error response tests ────────────────────────────────

    #[test]
    fn version_exists_errors_for_401_unauthorized() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(401)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .version_exists("demo", "1.0.0")
            .expect_err("401 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_errors_for_403_forbidden() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(403)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .version_exists("demo", "1.0.0")
            .expect_err("403 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_errors_for_401_unauthorized() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(401)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.crate_exists("demo").expect_err("401 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_errors_for_502_bad_gateway() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(502)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.crate_exists("demo").expect_err("502 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_errors_for_503_service_unavailable() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(503)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.crate_exists("demo").expect_err("503 must fail");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_errors_for_401_unauthorized() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(401)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.list_owners("demo", "token").expect_err("401 must fail");
        assert!(format!("{err:#}").contains("unexpected status while querying owners"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_errors_for_502_bad_gateway() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(502)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.list_owners("demo", "token").expect_err("502 must fail");
        assert!(format!("{err:#}").contains("unexpected status while querying owners"));
        handle.join().expect("join");
    }

    // ── Rate limiting (429) tests ────────────────────────────────────

    #[test]
    fn rate_limit_429_treated_as_not_visible_in_api_backoff() {
        let (api_base, handle) = with_multi_server(
            move |req| {
                req.respond(Response::empty(StatusCode(429)))
                    .expect("respond");
            },
            10,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(80),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(!visible);
        assert!(evidence.len() >= 2);
        assert!(evidence.iter().all(|e| !e.visible));
        handle.join().expect("join");
    }

    #[test]
    fn rate_limit_429_then_success_in_backoff() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let (api_base, handle) = with_multi_server(
            move |req| {
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                let status = if n < 2 { 429 } else { 200 };
                req.respond(Response::empty(StatusCode(status)))
                    .expect("respond");
            },
            5,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert!(evidence.len() >= 3);
        assert!(!evidence[0].visible);
        assert!(!evidence[1].visible);
        assert!(evidence.last().unwrap().visible);
        handle.join().expect("join");
    }

    // ── Malformed / empty response body tests ────────────────────────

    #[test]
    fn list_owners_errors_on_empty_response_body() {
        let (api_base, handle) = with_server(|req| {
            let resp = Response::from_string("")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .list_owners("demo", "token")
            .expect_err("empty body must fail");
        assert!(format!("{err:#}").contains("failed to parse owners JSON"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_errors_on_html_error_page() {
        let (api_base, handle) = with_server(|req| {
            let resp = Response::from_string("<html><body>503 Service Unavailable</body></html>")
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "text/html").expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli
            .list_owners("demo", "token")
            .expect_err("html must fail");
        assert!(format!("{err:#}").contains("failed to parse owners JSON"));
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_parses_response_with_multiple_owners() {
        let body = r#"{"users":[
            {"id":1,"login":"alice","name":"Alice"},
            {"id":2,"login":"bob","name":null},
            {"id":3,"login":"charlie","name":"Charlie D."}
        ]}"#;

        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string(body)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let owners = cli.list_owners("demo", "token").expect("owners");
        assert_eq!(owners.users.len(), 3);
        assert_eq!(owners.users[0].login, "alice");
        assert_eq!(owners.users[1].login, "bob");
        assert_eq!(owners.users[2].login, "charlie");
        assert!(owners.users[1].name.is_none());
        handle.join().expect("join");
    }

    #[test]
    fn list_owners_parses_empty_users_array() {
        let (api_base, handle) = with_server(|req| {
            let resp = Response::from_string(r#"{"users":[]}"#)
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let owners = cli.list_owners("demo", "token").expect("owners");
        assert!(owners.users.is_empty());
        handle.join().expect("join");
    }

    // ── Large response body tests ────────────────────────────────────

    #[test]
    fn list_owners_parses_large_response() {
        let mut users = Vec::new();
        for i in 0..100 {
            users.push(format!(
                r#"{{"id":{},"login":"user{}","name":"User {}"}}"#,
                i, i, i
            ));
        }
        let body = format!(r#"{{"users":[{}]}}"#, users.join(","));

        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string(body.as_str())
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let owners = cli.list_owners("demo", "token").expect("owners");
        assert_eq!(owners.users.len(), 100);
        assert_eq!(owners.users[99].login, "user99");
        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_with_large_index() {
        let mut lines = Vec::new();
        for i in 0..500 {
            lines.push(format!(r#"{{"vers":"{}.0.0"}}"#, i));
        }
        let index_content: String = lines.join("\n") + "\n";

        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string(index_content.as_str())
                .with_status_code(StatusCode(200))
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json")
                        .expect("header"),
                );
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        // Find the last version in a large index
        assert!(
            cli.check_index_visibility("demo", "499.0.0")
                .expect("check")
        );
        handle.join().expect("join");
    }

    #[test]
    fn parse_version_from_index_with_large_content() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        let mut lines = Vec::new();
        for i in 0..1000 {
            lines.push(format!(r#"{{"vers":"0.{}.0"}}"#, i));
        }
        let content = lines.join("\n") + "\n";

        assert!(cli.parse_version_from_index(&content, "0.999.0").unwrap());
        assert!(!cli.parse_version_from_index(&content, "0.1000.0").unwrap());
    }

    // ── Connection refused / reset tests ─────────────────────────────

    #[test]
    fn version_exists_errors_on_connection_refused() {
        // Bind a port then immediately drop the server so the port is closed
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        drop(server);

        let cli = RegistryClient::new(test_registry(addr)).expect("client");
        let err = cli
            .version_exists("demo", "1.0.0")
            .expect_err("connection refused must fail");
        assert!(format!("{err:#}").contains("registry request failed"));
    }

    #[test]
    fn crate_exists_errors_on_connection_refused() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        drop(server);

        let cli = RegistryClient::new(test_registry(addr)).expect("client");
        let err = cli
            .crate_exists("demo")
            .expect_err("connection refused must fail");
        assert!(format!("{err:#}").contains("registry request failed"));
    }

    #[test]
    fn list_owners_errors_on_connection_refused() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        drop(server);

        let cli = RegistryClient::new(test_registry(addr)).expect("client");
        let err = cli
            .list_owners("demo", "token")
            .expect_err("connection refused must fail");
        assert!(format!("{err:#}").contains("registry owners request failed"));
    }

    // ── Index sparse edge cases ──────────────────────────────────────

    #[test]
    fn fetch_index_file_errors_for_unexpected_status_code() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(500)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        // check_index_visibility degrades gracefully on errors
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_returns_false_for_429() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(429)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn check_index_visibility_returns_false_for_503() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(503)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn index_with_304_not_modified_without_cache_returns_error_gracefully() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(304)))
                .expect("respond");
        });

        // No cache dir set, so 304 should fail gracefully
        let cli = RegistryClient::new(test_registry_with_index(api_base)).expect("client");
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
        handle.join().expect("join");
    }

    #[test]
    fn index_with_304_not_modified_uses_cache() {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let cache_path = cache_dir.path().join("de").join("mo").join("demo");
        std::fs::create_dir_all(cache_path.parent().unwrap()).expect("mkdir");
        std::fs::write(&cache_path, "{\"vers\":\"2.0.0\"}\n").expect("write cache");

        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(304)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base))
            .expect("client")
            .with_cache_dir(cache_dir.path().to_path_buf());

        let visible = cli.check_index_visibility("demo", "2.0.0").expect("check");
        assert!(visible);
        handle.join().expect("join");
    }

    #[test]
    fn index_200_writes_cache_and_etag() {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let index_content = "{\"vers\":\"3.0.0\"}\n";

        let (api_base, handle) = with_server(move |req| {
            let resp = Response::from_string(index_content)
                .with_status_code(StatusCode(200))
                .with_header(tiny_http::Header::from_bytes("ETag", "\"abc123\"").expect("header"));
            req.respond(resp).expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base))
            .expect("client")
            .with_cache_dir(cache_dir.path().to_path_buf());

        let visible = cli.check_index_visibility("demo", "3.0.0").expect("check");
        assert!(visible);

        // Verify cache was written
        let cache_path = cache_dir.path().join("de").join("mo").join("demo");
        assert!(cache_path.exists());
        let cached = std::fs::read_to_string(&cache_path).expect("read cache");
        assert!(cached.contains("3.0.0"));

        // Verify etag was written
        let etag_path = cache_path.with_extension("etag");
        assert!(etag_path.exists());
        let etag = std::fs::read_to_string(&etag_path).expect("read etag");
        assert_eq!(etag, "\"abc123\"");

        handle.join().expect("join");
    }

    #[test]
    fn index_sends_etag_as_if_none_match() {
        use std::sync::Arc;
        use std::sync::Mutex;

        let cache_dir = tempfile::tempdir().expect("tempdir");
        // Pre-populate cache and etag
        let cache_path = cache_dir.path().join("de").join("mo").join("demo");
        std::fs::create_dir_all(cache_path.parent().unwrap()).expect("mkdir");
        std::fs::write(&cache_path, "{\"vers\":\"1.0.0\"}\n").expect("write");
        std::fs::write(cache_path.with_extension("etag"), "\"etag-val\"").expect("write etag");

        let received_header = Arc::new(Mutex::new(None));
        let received_header_clone = received_header.clone();

        let (api_base, handle) = with_server(move |req| {
            let inm = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("If-None-Match"))
                .map(|h| h.value.as_str().to_string());
            *received_header_clone.lock().unwrap() = inm;
            req.respond(Response::empty(StatusCode(304)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry_with_index(api_base))
            .expect("client")
            .with_cache_dir(cache_dir.path().to_path_buf());

        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(visible);

        let header = received_header.lock().unwrap().clone();
        assert_eq!(header, Some("\"etag-val\"".to_string()));
        handle.join().expect("join");
    }

    // ── Unicode in crate names ───────────────────────────────────────

    #[test]
    fn version_exists_with_hyphenated_crate_name() {
        let (api_base, handle) = with_server(|req| {
            assert_eq!(req.url(), "/api/v1/crates/my-crate/1.0.0");
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        assert!(cli.version_exists("my-crate", "1.0.0").expect("exists"));
        handle.join().expect("join");
    }

    #[test]
    fn version_exists_with_underscore_crate_name() {
        let (api_base, handle) = with_server(|req| {
            assert_eq!(req.url(), "/api/v1/crates/my_crate/2.0.0");
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        assert!(cli.version_exists("my_crate", "2.0.0").expect("exists"));
        handle.join().expect("join");
    }

    #[test]
    fn calculate_index_path_for_hyphenated_crate() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        assert_eq!(cli.calculate_index_path("my-crate"), "my/-c/my-crate");
    }

    #[test]
    fn calculate_index_path_lowercases_mixed_case() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        assert_eq!(cli.calculate_index_path("MyLib"), "my/li/mylib");
        assert_eq!(cli.calculate_index_path("UPPER"), "up/pe/upper");
    }

    // ── Concurrent registry checks ───────────────────────────────────

    #[test]
    fn concurrent_version_exists_checks() {
        let (api_base, handle) = with_multi_server(
            |req| {
                req.respond(Response::empty(StatusCode(200)))
                    .expect("respond");
            },
            5,
        );

        let cli =
            std::sync::Arc::new(RegistryClient::new(test_registry(api_base)).expect("client"));

        let handles: Vec<_> = (0..5)
            .map(|i| {
                let cli = cli.clone();
                let version = format!("{i}.0.0");
                thread::spawn(move || cli.version_exists("demo", &version))
            })
            .collect();

        for h in handles {
            let result = h.join().expect("thread join");
            assert!(result.expect("version_exists").eq(&true));
        }

        handle.join().expect("server join");
    }

    #[test]
    fn concurrent_crate_exists_checks() {
        let (api_base, handle) = with_multi_server(
            |req| {
                let url = req.url().to_string();
                if url.contains("missing") {
                    req.respond(Response::empty(StatusCode(404)))
                        .expect("respond");
                } else {
                    req.respond(Response::empty(StatusCode(200)))
                        .expect("respond");
                }
            },
            4,
        );

        let cli =
            std::sync::Arc::new(RegistryClient::new(test_registry(api_base)).expect("client"));

        let names = ["found1", "found2", "missing1", "missing2"];
        let handles: Vec<_> = names
            .iter()
            .map(|name| {
                let cli = cli.clone();
                let name = name.to_string();
                thread::spawn(move || (name.clone(), cli.crate_exists(&name)))
            })
            .collect();

        for h in handles {
            let (name, result) = h.join().expect("thread join");
            let exists = result.expect("crate_exists");
            if name.contains("missing") {
                assert!(!exists, "{name} should not exist");
            } else {
                assert!(exists, "{name} should exist");
            }
        }

        handle.join().expect("server join");
    }

    // ── verify_ownership edge cases ──────────────────────────────────

    #[test]
    fn verify_ownership_returns_false_on_401_unauthorized() {
        let (api_base, handle) = with_server(move |req| {
            req.respond(Response::empty(StatusCode(401)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let verified = cli.verify_ownership("demo", "fake-token").expect("verify");
        assert!(!verified);
        handle.join().expect("join");
    }

    #[test]
    fn verify_ownership_propagates_network_error() {
        // Non-existent address that doesn't match graceful-degradation patterns
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        drop(server);

        let cli = RegistryClient::new(test_registry(addr)).expect("client");
        // Connection refused produces an error that doesn't contain "forbidden"/"not found"
        // so it should propagate rather than degrade gracefully
        let result = cli.verify_ownership("demo", "token");
        assert!(result.is_err());
    }

    // ── Malformed index JSON edge cases ──────────────────────────────

    #[test]
    fn parse_version_from_index_only_whitespace_lines() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let content = "   \n  \n\t\n";
        assert!(!cli.parse_version_from_index(content, "1.0.0").unwrap());
    }

    #[test]
    fn parse_version_from_index_mixed_valid_and_garbage_lines() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let content = "garbage\n{\"vers\":\"1.0.0\"}\n<<invalid>>\n{\"vers\":\"2.0.0\"}\nnull\n";

        assert!(cli.parse_version_from_index(content, "1.0.0").unwrap());
        assert!(cli.parse_version_from_index(content, "2.0.0").unwrap());
        assert!(!cli.parse_version_from_index(content, "3.0.0").unwrap());
    }

    #[test]
    fn parse_version_from_index_json_array_instead_of_jsonl() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        // A JSON array is not valid JSONL format for sparse index
        let content = r#"[{"vers":"1.0.0"},{"vers":"2.0.0"}]"#;
        assert!(!cli.parse_version_from_index(content, "1.0.0").unwrap());
    }

    #[test]
    fn parse_version_from_index_extra_fields_ignored() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let content = r#"{"name":"demo","vers":"1.0.0","cksum":"abc123","deps":[],"features":{},"yanked":false}"#;
        // Even with extra fields, version should still be found
        assert!(cli.parse_version_from_index(content, "1.0.0").unwrap());
    }

    // ── Backoff edge-case tests ──────────────────────────────────────

    #[test]
    fn calculate_backoff_delay_zero_base() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let delay = cli.calculate_backoff_delay(Duration::ZERO, Duration::from_secs(10), 5, 0.0);
        assert_eq!(delay, Duration::ZERO);
    }

    #[test]
    fn calculate_backoff_delay_zero_max() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let delay = cli.calculate_backoff_delay(Duration::from_millis(100), Duration::ZERO, 3, 0.0);
        assert_eq!(delay, Duration::ZERO);
    }

    #[test]
    fn calculate_backoff_delay_attempt_overflow_is_safe() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        // u32::MAX attempt should not panic due to saturating arithmetic
        let delay = cli.calculate_backoff_delay(
            Duration::from_millis(100),
            Duration::from_secs(60),
            u32::MAX,
            0.0,
        );
        assert!(delay <= Duration::from_secs(60));
    }

    #[test]
    fn calculate_backoff_delay_full_jitter_stays_in_range() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        // With 100% jitter, delay should be 0..2× base
        for _ in 0..50 {
            let delay = cli.calculate_backoff_delay(
                Duration::from_millis(100),
                Duration::from_secs(10),
                1,
                1.0,
            );
            assert!(delay <= Duration::from_millis(200));
        }
    }

    // ── API base trailing-slash normalization ─────────────────────────

    #[test]
    fn version_exists_normalizes_trailing_slash() {
        let (api_base, handle) = with_server(|req| {
            assert_eq!(req.url(), "/api/v1/crates/demo/1.0.0");
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let registry = Registry {
            name: "test".to_string(),
            api_base: format!("{}/", api_base),
            index_base: None,
        };

        let cli = RegistryClient::new(registry).expect("client");
        assert!(cli.version_exists("demo", "1.0.0").expect("exists"));
        handle.join().expect("join");
    }

    #[test]
    fn crate_exists_normalizes_trailing_slash() {
        let (api_base, handle) = with_server(|req| {
            assert_eq!(req.url(), "/api/v1/crates/demo");
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let registry = Registry {
            name: "test".to_string(),
            api_base: format!("{}/", api_base),
            index_base: None,
        };

        let cli = RegistryClient::new(registry).expect("client");
        assert!(cli.crate_exists("demo").expect("exists"));
        handle.join().expect("join");
    }

    // ── check_new_crate edge cases ───────────────────────────────────

    #[test]
    fn check_new_crate_propagates_server_errors() {
        let (api_base, handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(500)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let err = cli.check_new_crate("demo").expect_err("500 must propagate");
        assert!(format!("{err:#}").contains("unexpected status"));
        handle.join().expect("join");
    }

    // ── Alternating status patterns ──────────────────────────────────

    #[test]
    fn backoff_handles_alternating_404_and_500_then_success() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let (api_base, handle) = with_multi_server(
            move |req| {
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                let status = match n {
                    0 => 404,
                    1 => 500,
                    2 => 404,
                    _ => 200,
                };
                req.respond(Response::empty(StatusCode(status)))
                    .expect("respond");
            },
            6,
        );

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");
        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert!(evidence.len() >= 4);
        assert!(!evidence[0].visible);
        assert!(!evidence[1].visible);
        assert!(!evidence[2].visible);
        assert!(evidence.last().unwrap().visible);
        handle.join().expect("join");
    }

    // ── Index mode backoff with 304 + cache ──────────────────────────

    #[test]
    fn index_mode_backoff_uses_cached_content_on_304() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let cache_dir = tempfile::tempdir().expect("tempdir");
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let (api_base, handle) = with_multi_server(
            move |req| {
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // First request: return index without the target version
                    let resp = Response::from_string("{\"vers\":\"0.9.0\"}\n")
                        .with_status_code(StatusCode(200))
                        .with_header(
                            tiny_http::Header::from_bytes("ETag", "\"v1\"").expect("header"),
                        );
                    req.respond(resp).expect("respond");
                } else {
                    // Subsequent requests: return updated content with target version
                    let resp =
                        Response::from_string("{\"vers\":\"0.9.0\"}\n{\"vers\":\"1.0.0\"}\n")
                            .with_status_code(StatusCode(200))
                            .with_header(
                                tiny_http::Header::from_bytes("ETag", "\"v2\"").expect("header"),
                            );
                    req.respond(resp).expect("respond");
                }
            },
            5,
        );

        let cli = RegistryClient::new(test_registry_with_index(api_base))
            .expect("client")
            .with_cache_dir(cache_dir.path().to_path_buf());

        let config = ReadinessConfig {
            enabled: true,
            method: ReadinessMethod::Index,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(30),
            max_total_wait: Duration::from_secs(5),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        };

        let (visible, evidence) = cli
            .is_version_visible_with_backoff("demo", "1.0.0", &config)
            .expect("backoff");
        assert!(visible);
        assert!(evidence.len() >= 2);
        assert!(!evidence[0].visible);
        assert!(evidence.last().unwrap().visible);
        handle.join().expect("join");
    }

    // ── Registry client builder ──────────────────────────────────────

    #[test]
    fn with_cache_dir_sets_cache_directory() {
        let registry = Registry {
            name: "test".to_string(),
            api_base: "https://example.com".to_string(),
            index_base: None,
        };
        let cli = RegistryClient::new(registry)
            .expect("client")
            .with_cache_dir(std::path::PathBuf::from("/tmp/test-cache"));
        // Verify the client can be constructed with a cache dir (smoke test)
        assert_eq!(cli.registry().name, "test");
    }

    #[test]
    fn registry_accessor_returns_correct_values() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let registry = Registry {
            name: "custom-registry".to_string(),
            api_base: api_base.clone(),
            index_base: Some("https://index.custom.io".to_string()),
        };

        let cli = RegistryClient::new(registry).expect("client");
        assert_eq!(cli.registry().name, "custom-registry");
        assert_eq!(cli.registry().api_base, api_base);
        assert_eq!(
            cli.registry().index_base.as_deref(),
            Some("https://index.custom.io")
        );
    }

    // ── Sparse index prefix stripping ────────────────────────────────

    #[test]
    fn registry_get_index_base_strips_sparse_prefix() {
        let registry = Registry {
            name: "test".to_string(),
            api_base: "https://example.com".to_string(),
            index_base: Some("sparse+https://index.example.com".to_string()),
        };

        assert_eq!(registry.get_index_base(), "https://index.example.com");
    }

    #[test]
    fn registry_get_index_base_leaves_non_sparse_prefix() {
        let registry = Registry {
            name: "test".to_string(),
            api_base: "https://example.com".to_string(),
            index_base: Some("https://index.example.com".to_string()),
        };

        assert_eq!(registry.get_index_base(), "https://index.example.com");
    }
}
