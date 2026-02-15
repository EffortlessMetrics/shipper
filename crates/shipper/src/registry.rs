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
}

impl RegistryClient {
    pub fn new(registry: Registry) -> Result<Self> {
        let http = Client::builder()
            .user_agent(format!("shipper/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self { registry, http })
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

    /// Calculate the index path for a crate using the 2+2+N scheme.
    ///
    /// For example, "serde" becomes "s/e/serde"
    /// and "tokio" becomes "t/o/tokio"
    fn calculate_index_path(&self, crate_name: &str) -> String {
        let chars: Vec<char> = crate_name.chars().collect();
        let first = if !chars.is_empty() { chars[0] } else { '_' };
        let second = if chars.len() > 1 { chars[1] } else { '_' };

        // Handle special cases: crates starting with non-alphanumeric characters
        let first = if first.is_alphanumeric() { first } else { '_' };
        let second = if second.is_alphanumeric() {
            second
        } else {
            '_'
        };

        format!("{}/{}/{}", first, second, crate_name)
    }

    /// Fetch the index file content from the registry.
    fn fetch_index_file(&self, index_path: &str) -> Result<String> {
        let index_base = self.registry.get_index_base();
        let url = format!("{}/{}", index_base.trim_end_matches('/'), index_path);

        let resp = self.http.get(&url).send().context("index request failed")?;

        match resp.status() {
            StatusCode::OK => {
                let content = resp.text().context("failed to read index response body")?;
                Ok(content)
            }
            StatusCode::NOT_FOUND => {
                // The crate doesn't exist in the index yet
                bail!("index file not found: {}", url)
            }
            s => bail!("unexpected status while fetching index: {}", s),
        }
    }

    /// Parse the index JSON and check if the version exists.
    fn parse_version_from_index(&self, content: &str, version: &str) -> Result<bool> {
        // The sparse index format is a JSON array of version objects
        #[derive(Deserialize)]
        struct IndexVersion {
            #[allow(dead_code)]
            #[serde(rename = "name")]
            _name: String,
            vers: String,
        }

        let versions: Vec<IndexVersion> = serde_json::from_str(content)
            .with_context(|| format!("failed to parse index JSON for version {}", version))?;

        Ok(versions.iter().any(|v| v.vers == version))
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
                let msg = e.to_string();
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

        // Test standard crate names
        assert_eq!(cli.calculate_index_path("serde"), "s/e/serde");
        assert_eq!(cli.calculate_index_path("tokio"), "t/o/tokio");
        assert_eq!(cli.calculate_index_path("rand"), "r/a/rand");
        assert_eq!(cli.calculate_index_path("http"), "h/t/http");
    }

    #[test]
    fn calculate_index_path_for_short_crate() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        // Test single-character crate name
        assert_eq!(cli.calculate_index_path("a"), "a/_/a");

        // Test two-character crate name
        assert_eq!(cli.calculate_index_path("ab"), "a/b/ab");
    }

    #[test]
    fn calculate_index_path_for_special_chars() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        // Test crate names starting with special characters
        assert_eq!(cli.calculate_index_path("_serde"), "_/s/_serde");
        assert_eq!(cli.calculate_index_path("-tokio"), "_/t/-tokio");
    }

    #[test]
    fn parse_version_from_index_finds_version() {
        let (api_base, _handle) = with_server(|req| {
            req.respond(Response::empty(StatusCode(200)))
                .expect("respond");
        });

        let cli = RegistryClient::new(test_registry(api_base)).expect("client");

        let index_content = r#"[
            {"name":"serde","vers":"1.0.0","deps":[],"cksum":"abc123"},
            {"name":"serde","vers":"1.0.1","deps":[],"cksum":"def456"},
            {"name":"serde","vers":"2.0.0","deps":[],"cksum":"ghi789"}
        ]"#;

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

        let index_content = r#"[
            {"name":"serde","vers":"1.0.0","deps":[],"cksum":"abc123"},
            {"name":"serde","vers":"1.0.1","deps":[],"cksum":"def456"}
        ]"#;

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
        assert!(found.is_err());
    }

    #[test]
    fn check_index_visibility_returns_true_for_existing_version() {
        let index_content = r#"[
            {"name":"demo","vers":"1.0.0","deps":[],"cksum":"abc123"},
            {"name":"demo","vers":"1.0.1","deps":[],"cksum":"def456"}
        ]"#;

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/d/e/demo");
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
        let index_content = r#"[
            {"name":"demo","vers":"1.0.0","deps":[],"cksum":"abc123"}
        ]"#;

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/d/e/demo");
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
        let index_content = r#"[
            {"name":"demo","vers":"1.0.0","deps":[],"cksum":"abc123"}
        ]"#;

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/d/e/demo");
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
        let index_content = r#"[
            {"name":"demo","vers":"1.0.0","deps":[],"cksum":"abc123"}
        ]"#;

        let (api_base, handle) = with_server(move |req| {
            assert_eq!(req.url(), "/d/e/demo");
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
        let index_content = "[]";

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
        let index_content = r#"[
            {"name":"demo","vers":"0.1.0","deps":[],"cksum":"abc123"},
            {"name":"demo","vers":"0.2.0","deps":[],"cksum":"def456"},
            {"name":"demo","vers":"1.0.0","deps":[],"cksum":"ghi789"},
            {"name":"demo","vers":"1.1.0","deps":[],"cksum":"jkl012"}
        ]"#;

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
        let malformed_json = r#"[
            {"name":"demo","vers":"1.0.0","deps":[],"cksum":"abc123"},
            {"invalid":"entry"}
        ]"#;

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
        // Should return false for malformed JSON (graceful degradation)
        let visible = cli.check_index_visibility("demo", "1.0.0").expect("check");
        assert!(!visible);
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
}
