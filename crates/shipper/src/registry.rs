use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::{Duration, Instant};

use crate::types::{ReadinessConfig, ReadinessMethod, Registry};

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

    /// Check if a version is visible with exponential backoff and jitter.
    ///
    /// Returns Ok(true) if the version becomes visible within the timeout,
    /// Ok(false) if the timeout is exceeded, or Err on other failures.
    pub fn is_version_visible_with_backoff(
        &self,
        crate_name: &str,
        version: &str,
        config: &ReadinessConfig,
    ) -> Result<bool> {
        if !config.enabled {
            // If readiness checks are disabled, just check once
            return self.version_exists(crate_name, version);
        }

        let start = Instant::now();
        let mut attempt: u32 = 0;

        // Initial delay before first poll
        if config.initial_delay > Duration::ZERO {
            std::thread::sleep(config.initial_delay);
        }

        loop {
            attempt += 1;

            // Check visibility based on method
            let visible = match config.method {
                ReadinessMethod::Api => self.version_exists(crate_name, version)?,
                ReadinessMethod::Index => {
                    // Index-based verification (future-proofing)
                    // For now, fall back to API check
                    self.version_exists(crate_name, version)?
                }
                ReadinessMethod::Both => {
                    // Check both API and index
                    // For now, just use API result
                    self.version_exists(crate_name, version)?
                }
            };

            if visible {
                return Ok(true);
            }

            // Check if we've exceeded max total wait
            if start.elapsed() >= config.max_total_wait {
                return Ok(false);
            }

            // Calculate next delay with exponential backoff and jitter
            let base_delay = config.poll_interval;
            let exponential_delay =
                base_delay.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1).min(16)));
            let capped_delay = exponential_delay.min(config.max_delay);

            // Apply jitter: delay * (1 ± jitter_factor)
            // Using rand::random() like the existing backoff_delay function
            let jitter_range = config.jitter_factor;
            let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_range - jitter_range);
            let jittered_delay =
                Duration::from_millis((capped_delay.as_millis() as f64 * jitter).round() as u64);

            std::thread::sleep(jittered_delay);
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
        }
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
        };

        let result = cli.is_version_visible_with_backoff("demo", "1.0.0", &config);
        assert!(result.is_ok());
        assert!(result.unwrap());
        handle.join().expect("join");
    }
}
