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

pub(crate) fn to_micro_payload(payload: &WebhookPayload) -> shipper_webhook::WebhookPayload {
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

#[cfg(test)]
mod tests {
    use super::*;
    use shipper_webhook::WebhookType;

    fn sample_config(url: &str) -> WebhookConfig {
        WebhookConfig {
            url: url.to_string(),
            webhook_type: WebhookType::Generic,
            secret: None,
            timeout_secs: 30,
        }
    }

    fn started_event() -> WebhookEvent {
        WebhookEvent::PublishStarted {
            plan_id: "plan-1".to_string(),
            package_count: 3,
            registry: "crates-io".to_string(),
        }
    }

    fn succeeded_event() -> WebhookEvent {
        WebhookEvent::PublishSucceeded {
            plan_id: "plan-1".to_string(),
            package_name: "pkg-a".to_string(),
            package_version: "1.2.3".to_string(),
            duration_ms: 1500,
        }
    }

    fn failed_event() -> WebhookEvent {
        WebhookEvent::PublishFailed {
            plan_id: "plan-1".to_string(),
            package_name: "pkg-b".to_string(),
            package_version: "0.1.0".to_string(),
            error_class: "Permanent".to_string(),
            message: "auth failed".to_string(),
        }
    }

    fn completed_event(failure_count: usize) -> WebhookEvent {
        WebhookEvent::PublishCompleted {
            plan_id: "plan-1".to_string(),
            total_packages: 4,
            success_count: 4 - failure_count,
            failure_count,
            skipped_count: 0,
            result: if failure_count == 0 {
                "success".to_string()
            } else {
                "partial".to_string()
            },
        }
    }

    fn sample_payload(event: WebhookEvent) -> WebhookPayload {
        WebhookPayload {
            timestamp: Utc::now(),
            event,
        }
    }

    #[test]
    fn webhook_client_new_rejects_empty_url() {
        let cfg = sample_config("");
        let err = match WebhookClient::new(&cfg) {
            Err(e) => e,
            Ok(_) => panic!("empty url must be rejected"),
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("required"), "unexpected error: {msg}");
    }

    #[test]
    fn webhook_client_new_rejects_whitespace_only_url() {
        let cfg = sample_config("   ");
        let err = match WebhookClient::new(&cfg) {
            Err(e) => e,
            Ok(_) => panic!("whitespace url must be rejected"),
        };
        assert!(format!("{err:#}").contains("required"));
    }

    #[test]
    fn webhook_client_new_accepts_valid_url() {
        let cfg = sample_config("https://example.invalid/hook");
        WebhookClient::new(&cfg).expect("valid url must be accepted");
    }

    #[test]
    fn webhook_client_is_clone() {
        let cfg = sample_config("https://example.invalid/hook");
        let client = WebhookClient::new(&cfg).expect("ok");
        let _ = client.clone();
    }

    #[test]
    fn maybe_send_event_returns_early_on_empty_url() {
        let cfg = sample_config("");
        maybe_send_event(&cfg, started_event());
    }

    #[test]
    fn maybe_send_event_returns_early_on_whitespace_url() {
        let cfg = sample_config(" \t ");
        maybe_send_event(&cfg, succeeded_event());
    }

    #[test]
    fn to_micro_payload_publish_started_fields() {
        let micro = to_micro_payload(&sample_payload(started_event()));
        assert!(micro.success);
        assert_eq!(micro.title.as_deref(), Some("Publish Started"));
        assert!(micro.package.is_none());
        assert!(micro.version.is_none());
        assert_eq!(micro.registry.as_deref(), Some("crates-io"));
        assert!(micro.error.is_none());
        assert!(micro.message.contains("plan-1"));
        assert!(micro.message.contains("3 packages"));
        assert!(micro.message.contains("crates-io"));
        let legacy = micro.extra.get("legacy").expect("legacy");
        assert_eq!(
            legacy.get("event").and_then(|v| v.as_str()),
            Some("publish_started")
        );
        assert_eq!(
            legacy.get("plan_id").and_then(|v| v.as_str()),
            Some("plan-1")
        );
        assert_eq!(
            legacy.get("package_count").and_then(|v| v.as_u64()),
            Some(3)
        );
        assert_eq!(
            legacy.get("registry").and_then(|v| v.as_str()),
            Some("crates-io")
        );
    }

    #[test]
    fn to_micro_payload_publish_succeeded_fields() {
        let micro = to_micro_payload(&sample_payload(succeeded_event()));
        assert!(micro.success);
        assert_eq!(micro.title.as_deref(), Some("Publish Succeeded"));
        assert_eq!(micro.package.as_deref(), Some("pkg-a"));
        assert_eq!(micro.version.as_deref(), Some("1.2.3"));
        assert!(micro.registry.is_none());
        assert!(micro.error.is_none());
        assert!(micro.message.contains("pkg-a"));
        assert!(micro.message.contains("1.2.3"));
        assert!(micro.message.contains("1500ms"));
        let legacy = micro.extra.get("legacy").expect("legacy");
        assert_eq!(
            legacy.get("event").and_then(|v| v.as_str()),
            Some("publish_succeeded")
        );
        assert_eq!(
            legacy.get("duration_ms").and_then(|v| v.as_u64()),
            Some(1500)
        );
    }

    #[test]
    fn to_micro_payload_publish_failed_fields() {
        let micro = to_micro_payload(&sample_payload(failed_event()));
        assert!(!micro.success);
        assert_eq!(micro.title.as_deref(), Some("Publish Failed"));
        assert_eq!(micro.package.as_deref(), Some("pkg-b"));
        assert_eq!(micro.version.as_deref(), Some("0.1.0"));
        assert!(micro.registry.is_none());
        assert_eq!(micro.error.as_deref(), Some("auth failed"));
        assert!(micro.message.contains("Permanent"));
        assert!(micro.message.contains("auth failed"));
        let legacy = micro.extra.get("legacy").expect("legacy");
        assert_eq!(
            legacy.get("event").and_then(|v| v.as_str()),
            Some("publish_failed")
        );
        assert_eq!(
            legacy.get("error_class").and_then(|v| v.as_str()),
            Some("Permanent")
        );
    }

    #[test]
    fn to_micro_payload_publish_completed_success_when_no_failures() {
        let micro = to_micro_payload(&sample_payload(completed_event(0)));
        assert!(micro.success, "no failures -> success=true");
        assert_eq!(micro.title.as_deref(), Some("Publish Completed"));
        assert!(micro.package.is_none());
        assert!(micro.version.is_none());
        assert!(micro.registry.is_none());
        assert!(micro.error.is_none());
        assert!(micro.message.contains("4/4 succeeded"));
        let legacy = micro.extra.get("legacy").expect("legacy");
        assert_eq!(
            legacy.get("total_packages").and_then(|v| v.as_u64()),
            Some(4)
        );
        assert_eq!(
            legacy.get("success_count").and_then(|v| v.as_u64()),
            Some(4)
        );
        assert_eq!(
            legacy.get("failure_count").and_then(|v| v.as_u64()),
            Some(0)
        );
        assert_eq!(
            legacy.get("result").and_then(|v| v.as_str()),
            Some("success")
        );
    }

    #[test]
    fn to_micro_payload_publish_completed_failure_when_any_failed() {
        let micro = to_micro_payload(&sample_payload(completed_event(2)));
        assert!(!micro.success, "any failures -> success=false");
        assert!(micro.message.contains("2/4 succeeded"));
        assert!(micro.message.contains("2 failed"));
        let legacy = micro.extra.get("legacy").expect("legacy");
        assert_eq!(
            legacy.get("failure_count").and_then(|v| v.as_u64()),
            Some(2)
        );
        assert_eq!(
            legacy.get("result").and_then(|v| v.as_str()),
            Some("partial")
        );
    }

    #[test]
    fn to_micro_payload_includes_legacy_key() {
        for event in [
            started_event(),
            succeeded_event(),
            failed_event(),
            completed_event(1),
        ] {
            let micro = to_micro_payload(&sample_payload(event));
            assert!(
                micro.extra.contains_key("legacy"),
                "to_micro_payload must always include `legacy` key"
            );
        }
    }

    #[test]
    fn webhook_event_serde_roundtrip_for_all_variants() {
        for event in [
            started_event(),
            succeeded_event(),
            failed_event(),
            completed_event(1),
        ] {
            let json = serde_json::to_string(&event).expect("serialize");
            let back: WebhookEvent = serde_json::from_str(&json).expect("deserialize");
            // Reuse serialization for structural equality.
            assert_eq!(
                json,
                serde_json::to_string(&back).expect("re-serialize"),
                "roundtrip must preserve event payload"
            );
        }
    }

    #[test]
    fn webhook_event_publish_started_tag_in_json() {
        let json = serde_json::to_string(&started_event()).expect("serialize");
        assert!(
            json.contains("\"event\":\"publish_started\""),
            "expected tagged variant, got {json}"
        );
    }

    #[test]
    fn webhook_event_publish_failed_tag_in_json() {
        let json = serde_json::to_string(&failed_event()).expect("serialize");
        assert!(
            json.contains("\"event\":\"publish_failed\""),
            "expected tagged variant, got {json}"
        );
    }

    #[test]
    fn webhook_payload_serializes_with_event_and_timestamp() {
        let payload = sample_payload(started_event());
        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(json.contains("\"timestamp\""));
        assert!(json.contains("\"event\":\"publish_started\""));
    }
}
