//! Webhook notifications backed by the `shipper-webhook` microcrate.
//!
//! This module keeps `shipper`'s public webhook API stable while delegating the
//! HTTP transport behavior to the dedicated microcrate.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Webhook configuration type provided by the `shipper-webhook` microcrate.
pub type WebhookConfig = shipper_webhook::WebhookConfig;

/// Webhook events published during a publish run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WebhookEvent {
    /// Publish workflow started.
    PublishStarted {
        plan_id: String,
        package_count: usize,
        registry: String,
    },
    /// A crate publish succeeded.
    PublishSucceeded {
        plan_id: String,
        package_name: String,
        package_version: String,
        duration_ms: u64,
    },
    /// A crate publish failed.
    PublishFailed {
        plan_id: String,
        package_name: String,
        package_version: String,
        error_class: String,
        message: String,
    },
    /// All publish operations completed.
    PublishCompleted {
        plan_id: String,
        total_packages: usize,
        success_count: usize,
        failure_count: usize,
        skipped_count: usize,
        result: String,
    },
}

/// Typed webhook payload payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Timestamp of the event (ISO 8601).
    pub timestamp: DateTime<Utc>,
    /// Event details.
    pub event: WebhookEvent,
}

/// Webhook client for dispatching publish events.
#[derive(Clone)]
pub struct WebhookClient {
    config: WebhookConfig,
}

impl WebhookClient {
    /// Create a webhook client with the given configuration.
    pub fn new(config: &WebhookConfig) -> Result<Self> {
        if config.url.trim().is_empty() {
            anyhow::bail!("webhook URL is required when webhooks are enabled");
        }
        Ok(Self {
            config: config.clone(),
        })
    }

    /// Send a webhook event asynchronously.
    pub fn send_event(&self, event: WebhookEvent) {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event,
        };

        let client = self.clone();
        let _ = std::thread::spawn(move || {
            if let Err(e) =
                shipper_webhook::send_webhook(&client.config, &to_micro_payload(&payload))
            {
                eprintln!("[warn] webhook delivery failed (non-blocking): {:#}", e);
            }
        });
    }
}

/// Send a webhook event if webhooks are configured.
pub fn maybe_send_event(config: &WebhookConfig, event: WebhookEvent) {
    if config.url.trim().is_empty() {
        return;
    }

    let client = match WebhookClient::new(config) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("[warn] failed to build webhook client: {:#}", e);
            return;
        }
    };

    let _ = std::thread::spawn(move || {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event,
        };

        if let Err(e) = shipper_webhook::send_webhook(&client.config, &to_micro_payload(&payload)) {
            eprintln!("[warn] webhook delivery failed (non-blocking): {:#}", e);
        }
    });
}

fn to_micro_payload(payload: &WebhookPayload) -> shipper_webhook::WebhookPayload {
    let (message, title, success, package, version, registry, error, extra) = match &payload.event {
        WebhookEvent::PublishStarted {
            plan_id,
            package_count,
            registry,
        } => (
            format!("publish started for plan {plan_id} ({package_count} packages) on {registry}"),
            Some("Publish Started".to_string()),
            true,
            None,
            None,
            Some(registry.clone()),
            None,
            serde_json::json!({
                "event": "publish_started",
                "plan_id": plan_id,
                "package_count": package_count,
                "registry": registry,
            }),
        ),
        WebhookEvent::PublishSucceeded {
            plan_id,
            package_name,
            package_version,
            duration_ms,
            ..
        } => (
            format!(
                "publish succeeded for package {package_name} version {package_version} in {duration_ms}ms (plan {plan_id})"
            ),
            Some("Publish Succeeded".to_string()),
            true,
            Some(package_name.clone()),
            Some(package_version.clone()),
            None,
            None,
            serde_json::json!({
                "event": "publish_succeeded",
                "plan_id": plan_id,
                "duration_ms": duration_ms,
            }),
        ),
        WebhookEvent::PublishFailed {
            plan_id,
            package_name,
            package_version,
            error_class,
            message,
            ..
        } => (
            format!(
                "publish failed for package {package_name} version {package_version} ({error_class}): {message}"
            ),
            Some("Publish Failed".to_string()),
            false,
            Some(package_name.clone()),
            Some(package_version.clone()),
            None,
            Some(message.clone()),
            serde_json::json!({
                "event": "publish_failed",
                "plan_id": plan_id,
                "error_class": error_class,
            }),
        ),
        WebhookEvent::PublishCompleted {
            plan_id,
            total_packages,
            success_count,
            failure_count,
            skipped_count,
            result,
        } => (
            format!(
                "publish completed: {success_count}/{total_packages} succeeded, {failure_count} failed, {skipped_count} skipped (plan {plan_id}, result: {result})"
            ),
            Some("Publish Completed".to_string()),
            *failure_count == 0,
            None,
            None,
            None,
            None,
            serde_json::json!({
                "event": "publish_completed",
                "plan_id": plan_id,
                "total_packages": total_packages,
                "success_count": success_count,
                "failure_count": failure_count,
            "skipped_count": skipped_count,
            "result": result,
            }),
        ),
    };

    let mut extra_fields = BTreeMap::new();
    extra_fields.insert("legacy".to_string(), extra);

    shipper_webhook::WebhookPayload {
        message,
        title,
        success,
        package,
        version,
        registry,
        error,
        extra: extra_fields,
    }
}
