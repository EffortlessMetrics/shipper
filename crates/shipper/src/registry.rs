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

        let resp = self.http.get(url).send().context("registry request failed")?;
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
            StatusCode::FORBIDDEN => bail!("forbidden when querying owners; token may be invalid or missing required scope"),
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
