//! Readiness visibility helpers for parallel publish.
//!
//! Checks whether a newly-published crate version is visible on the registry,
//! with exponential backoff and optional sparse-index fallback.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;

use shipper_registry::HttpRegistryClient as RegistryClient;
use shipper_types::{ReadinessConfig, ReadinessEvidence, ReadinessMethod};

/// Check readiness visibility with exponential backoff and optional sparse-index fallback.
pub(super) fn is_version_visible_with_backoff(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &ReadinessConfig,
) -> Result<(bool, Vec<ReadinessEvidence>)> {
    let mut evidence = Vec::new();

    if !config.enabled {
        let visible = reg.version_exists(crate_name, version)?;
        evidence.push(ReadinessEvidence {
            attempt: 1,
            visible,
            timestamp: Utc::now(),
            delay_before: Duration::ZERO,
        });
        return Ok((visible, evidence));
    }

    let start = Instant::now();
    let mut attempt = 0u32;

    if config.initial_delay > Duration::ZERO {
        thread::sleep(config.initial_delay);
    }

    loop {
        attempt += 1;

        let jittered_delay = if attempt == 1 {
            Duration::ZERO
        } else {
            let base_delay = config.poll_interval;
            let exponential_delay =
                base_delay.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(2).min(16)));
            let capped_delay = exponential_delay.min(config.max_delay);
            let jitter_range = config.jitter_factor;
            let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_range - jitter_range);
            Duration::from_millis((capped_delay.as_millis() as f64 * jitter).round() as u64)
        };

        let visible = match config.method {
            ReadinessMethod::Api => reg.version_exists(crate_name, version).unwrap_or(false),
            ReadinessMethod::Index => {
                is_version_visible_via_index(reg, crate_name, version, config).unwrap_or(false)
            }
            ReadinessMethod::Both => {
                if config.prefer_index {
                    if is_version_visible_via_index(reg, crate_name, version, config)
                        .unwrap_or(false)
                    {
                        true
                    } else {
                        reg.version_exists(crate_name, version).unwrap_or(false)
                    }
                } else if reg.version_exists(crate_name, version).unwrap_or(false) {
                    true
                } else {
                    is_version_visible_via_index(reg, crate_name, version, config).unwrap_or(false)
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
        thread::sleep(next_delay);
    }
}

fn is_version_visible_via_index(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &ReadinessConfig,
) -> Result<bool> {
    let content = if let Some(path) = &config.index_path {
        std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!(
                "failed to read local sparse-index path {}: {}",
                path.display(),
                e
            )
        })?
    } else {
        reg.fetch_sparse_index_file(reg.base_url(), crate_name)?
    };

    Ok(shipper_sparse_index::contains_version(&content, version))
}
