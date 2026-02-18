//! Webhook notifications for publish events.
//!
//! This module provides webhook functionality to send HTTP POST callbacks
//! when publish events occur. Webhooks are optional and disabled by default.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Webhook configuration settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookConfig {
    /// Enable webhook notifications (default: false - disabled)
    pub enabled: bool,
    /// The webhook URL to send POST requests to
    pub url: Option<String>,
    /// Optional secret for signing webhook payloads
    pub secret: Option<String>,
    /// Request timeout (default: 30 seconds)
    pub timeout: Duration,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: None,
            secret: None,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Events that can trigger webhook notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WebhookEvent {
    /// Publish operation started
    PublishStarted {
        plan_id: String,
        package_count: usize,
        registry: String,
    },
    /// A package publish succeeded
    PublishSucceeded {
        plan_id: String,
        package_name: String,
        package_version: String,
        duration_ms: u64,
    },
    /// A package publish failed
    PublishFailed {
        plan_id: String,
        package_name: String,
        package_version: String,
        error_class: String,
        message: String,
    },
    /// All publish operations completed
    PublishCompleted {
        plan_id: String,
        total_packages: usize,
        success_count: usize,
        failure_count: usize,
        skipped_count: usize,
        result: String,
    },
}

/// Payload structure for webhook POST requests.
#[derive(Debug, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Timestamp of the event (ISO 8601)
    pub timestamp: DateTime<Utc>,
    /// The event type and data
    pub event: WebhookEvent,
}

/// Webhook client for sending HTTP POST callbacks.
///
/// This client is designed to be fire-and-forget - failures are logged
/// but do not block the publishing process.
pub struct WebhookClient {
    client: reqwest::blocking::Client,
    url: String,
    secret: Option<String>,
}

impl WebhookClient {
    /// Create a new webhook client with the given configuration.
    pub fn new(config: &WebhookConfig) -> Result<Self> {
        let url = config
            .url
            .clone()
            .context("webhook URL is required when webhooks are enabled")?;

        let client = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .context("failed to build webhook HTTP client")?;

        Ok(Self {
            client,
            url,
            secret: config.secret.clone(),
        })
    }

    /// Send a webhook event.
    ///
    /// This method is designed to be non-blocking - it logs any errors
    /// but does not propagate them to avoid blocking the publish process.
    pub fn send_event(&self, event: WebhookEvent) {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event,
        };

        // Spawn a thread to handle the webhook request asynchronously
        // This ensures webhook failures don't block publishing
        let client = self.client.clone();
        let url = self.url.clone();
        let secret = self.secret.clone();

        std::thread::spawn(move || {
            if let Err(e) = do_send_event(&client, &url, secret.as_deref(), &payload) {
                eprintln!("[warn] webhook delivery failed (non-blocking): {:#}", e);
            }
        });
    }
}

fn do_send_event(
    client: &reqwest::blocking::Client,
    url: &str,
    secret: Option<&str>,
    payload: &WebhookPayload,
) -> Result<()> {
    let json = serde_json::to_string(payload).context("failed to serialize webhook payload")?;

    let mut request = client.post(url).header("Content-Type", "application/json");

    // Add signature header if secret is provided
    if let Some(secret) = secret {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        hasher.update(secret.as_bytes());
        let signature = hex::encode(hasher.finalize());
        request = request.header("X-Shipper-Signature", format!("sha256={}", signature));
    }

    let response = request
        .body(json)
        .send()
        .context("failed to send webhook request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        anyhow::bail!("webhook returned error status {}: {}", status, body);
    }

    Ok(())
}

/// Send a webhook event if webhooks are configured.
///
/// This is a convenience function that checks if webhooks are enabled
/// and sends the event if so. It returns silently if webhooks are disabled.
pub fn maybe_send_event(config: &WebhookConfig, event: WebhookEvent) {
    if !config.enabled {
        return;
    }

    // Skip if URL is not configured
    let url = match &config.url {
        Some(url) if !url.is_empty() => url,
        _ => return,
    };

    // Create a simple client inline for fire-and-forget delivery
    let client = match reqwest::blocking::Client::builder()
        .timeout(config.timeout)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[warn] failed to build webhook client: {:#}", e);
            return;
        }
    };

    let url = url.clone();
    let secret = config.secret.clone();

    std::thread::spawn(move || {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event,
        };

        if let Err(e) = do_send_event(&client, &url, secret.as_deref(), &payload) {
            eprintln!("[warn] webhook delivery failed (non-blocking): {:#}", e);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_config_defaults_are_disabled() {
        let config = WebhookConfig::default();
        assert!(!config.enabled);
        assert!(config.url.is_none());
        assert!(config.secret.is_none());
        assert_eq!(config.timeout, Duration::from_secs(30));
    }

    #[test]
    fn webhook_payload_serialization() {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event: WebhookEvent::PublishStarted {
                plan_id: "test-plan".to_string(),
                package_count: 3,
                registry: "crates-io".to_string(),
            },
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        let parsed: WebhookPayload = serde_json::from_str(&json).expect("deserialize");

        match parsed.event {
            WebhookEvent::PublishStarted {
                plan_id,
                package_count,
                registry,
            } => {
                assert_eq!(plan_id, "test-plan");
                assert_eq!(package_count, 3);
                assert_eq!(registry, "crates-io");
            }
            _ => panic!("unexpected event type"),
        }
    }

    #[test]
    fn webhook_client_requires_url() {
        let config = WebhookConfig {
            enabled: true,
            url: None,
            secret: None,
            timeout: Duration::from_secs(30),
        };

        let result = WebhookClient::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn webhook_event_serializations() {
        // Test all event types
        let events = vec![
            WebhookEvent::PublishStarted {
                plan_id: "plan-1".to_string(),
                package_count: 5,
                registry: "crates-io".to_string(),
            },
            WebhookEvent::PublishSucceeded {
                plan_id: "plan-1".to_string(),
                package_name: "mylib".to_string(),
                package_version: "1.0.0".to_string(),
                duration_ms: 5000,
            },
            WebhookEvent::PublishFailed {
                plan_id: "plan-1".to_string(),
                package_name: "mylib".to_string(),
                package_version: "1.0.0".to_string(),
                error_class: "permanent".to_string(),
                message: "permission denied".to_string(),
            },
            WebhookEvent::PublishCompleted {
                plan_id: "plan-1".to_string(),
                total_packages: 5,
                success_count: 4,
                failure_count: 1,
                skipped_count: 0,
                result: "partial_failure".to_string(),
            },
        ];

        for event in events {
            let payload = WebhookPayload {
                timestamp: Utc::now(),
                event,
            };

            let json = serde_json::to_string(&payload).expect("serialize");
            let parsed: WebhookPayload = serde_json::from_str(&json).expect("deserialize");
            assert!(matches!(
                parsed.event,
                WebhookEvent::PublishStarted { .. }
                    | WebhookEvent::PublishSucceeded { .. }
                    | WebhookEvent::PublishFailed { .. }
                    | WebhookEvent::PublishCompleted { .. }
            ));
        }
    }
}
