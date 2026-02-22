use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::types::{ReadinessConfig, ReadinessEvidence, ReadinessMethod, Registry};

#[derive(Debug, Clone)]
pub struct RegistryClient {
    registry: Registry,
    registry_client: shipper_registry::RegistryClient,
}

impl RegistryClient {
    pub fn new(registry: Registry) -> Result<Self> {
        let api_base = registry.api_base.trim_end_matches('/');
        Ok(Self {
            registry: registry.clone(),
            registry_client: shipper_registry::RegistryClient::new(api_base),
        })
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn version_exists(&self, crate_name: &str, version: &str) -> Result<bool> {
        self.registry_client
            .version_exists(crate_name, version)
            .or_else(|err| bail!("unexpected status while checking version existence: {err}"))
    }

    pub fn crate_exists(&self, crate_name: &str) -> Result<bool> {
        self.registry_client
            .crate_exists(crate_name)
            .or_else(|err| bail!("unexpected status while checking crate existence: {err}"))
    }

    pub fn list_owners(&self, crate_name: &str, token: &str) -> Result<OwnersResponse> {
        let response = self
            .registry_client
            .list_owners(crate_name, token)
            .context("registry owners request failed")?;

        Ok(OwnersResponse {
            users: response
                .users
                .into_iter()
                .map(|user| Owner {
                    id: user.id.unwrap_or_default(),
                    login: user.login,
                    name: user.name,
                })
                .collect(),
        })
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
        let index_base = self.registry.get_index_base();
        self.registry_client
            .is_version_visible_in_sparse_index(&index_base, crate_name, version)
            .or_else(|_| Ok(false))
    }

    /// Attempt ownership verification for a crate.
    ///
    /// Returns true if ownership is verified, false if verification fails or endpoint is unavailable.
    /// This function degrades gracefully: ownership endpoint failures return false.
    pub fn verify_ownership(&self, crate_name: &str, token: &str) -> Result<bool> {
        match self.list_owners(crate_name, token) {
            Ok(_) => Ok(true),
            Err(e) => {
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

        if config.initial_delay > Duration::ZERO {
            std::thread::sleep(config.initial_delay);
        }

        loop {
            attempt += 1;

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

            if start.elapsed() >= config.max_total_wait {
                return Ok((false, evidence));
            }

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

        let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_factor - jitter_factor);
        let millis = (delay.as_millis() as f64 * jitter).round() as u128;
        Duration::from_millis(millis as u64)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OwnersResponse {
    pub users: Vec<Owner>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Owner {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
}
