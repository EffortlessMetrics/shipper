use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::types::Registry;

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
}
