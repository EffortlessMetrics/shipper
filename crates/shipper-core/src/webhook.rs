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

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(event: WebhookEvent) -> WebhookPayload {
        WebhookPayload {
            timestamp: Utc::now(),
            event,
        }
    }

    fn legacy_field(out: &shipper_webhook::WebhookPayload) -> &serde_json::Value {
        out.extra
            .get("legacy")
            .expect("to_micro_payload always populates a `legacy` extra field")
    }

    // ---- WebhookClient::new ----

    #[test]
    fn webhook_client_new_rejects_empty_url() {
        let config = WebhookConfig {
            url: String::new(),
            ..Default::default()
        };

        match WebhookClient::new(&config) {
            Ok(_) => panic!("empty URL must be rejected"),
            Err(err) => {
                let msg = format!("{err:#}");
                assert!(
                    msg.contains("webhook URL is required"),
                    "unexpected error message: {msg}"
                );
            }
        }
    }

    #[test]
    fn webhook_client_new_rejects_whitespace_only_url() {
        let config = WebhookConfig {
            url: "   \t\n".to_string(),
            ..Default::default()
        };

        assert!(
            WebhookClient::new(&config).is_err(),
            "whitespace-only URL should be treated as empty"
        );
    }

    #[test]
    fn webhook_client_new_accepts_valid_url() {
        let config = WebhookConfig {
            url: "https://example.com/hook".to_string(),
            ..Default::default()
        };

        assert!(WebhookClient::new(&config).is_ok());
    }

    // ---- maybe_send_event ----

    #[test]
    fn maybe_send_event_empty_url_is_silent_noop() {
        // No HTTP client built, no thread spawned. Must not panic.
        maybe_send_event(
            &WebhookConfig::default(),
            WebhookEvent::PublishStarted {
                plan_id: "plan-1".to_string(),
                package_count: 1,
                registry: "crates-io".to_string(),
            },
        );
    }

    #[test]
    fn maybe_send_event_whitespace_url_is_silent_noop() {
        // Whitespace-only URL trips the same early-return path as empty URL.
        let config = WebhookConfig {
            url: "  ".to_string(),
            ..Default::default()
        };

        maybe_send_event(
            &config,
            WebhookEvent::PublishCompleted {
                plan_id: "plan-1".to_string(),
                total_packages: 1,
                success_count: 1,
                failure_count: 0,
                skipped_count: 0,
                result: "success".to_string(),
            },
        );
    }

    // ---- to_micro_payload — per-variant mapping ----

    #[test]
    fn to_micro_payload_publish_started_populates_registry_and_legacy() {
        let event = WebhookEvent::PublishStarted {
            plan_id: "plan-42".to_string(),
            package_count: 3,
            registry: "crates-io".to_string(),
        };
        let out = to_micro_payload(&payload(event));

        assert_eq!(out.title.as_deref(), Some("Publish Started"));
        assert!(out.success);
        assert!(out.package.is_none());
        assert!(out.version.is_none());
        assert_eq!(out.registry.as_deref(), Some("crates-io"));
        assert!(out.error.is_none());
        assert!(out.message.contains("plan-42"));
        assert!(out.message.contains("3 packages"));
        assert!(out.message.contains("crates-io"));

        let legacy = legacy_field(&out);
        assert_eq!(legacy["event"], "publish_started");
        assert_eq!(legacy["plan_id"], "plan-42");
        assert_eq!(legacy["package_count"], 3);
    }

    #[test]
    fn to_micro_payload_publish_succeeded_marks_success_with_package() {
        let event = WebhookEvent::PublishSucceeded {
            plan_id: "plan-7".to_string(),
            package_name: "my-crate".to_string(),
            package_version: "1.2.3".to_string(),
            duration_ms: 4321,
        };
        let out = to_micro_payload(&payload(event));

        assert_eq!(out.title.as_deref(), Some("Publish Succeeded"));
        assert!(out.success);
        assert_eq!(out.package.as_deref(), Some("my-crate"));
        assert_eq!(out.version.as_deref(), Some("1.2.3"));
        assert!(out.registry.is_none());
        assert!(out.error.is_none());
        assert!(out.message.contains("my-crate"));
        assert!(out.message.contains("1.2.3"));
        assert!(out.message.contains("4321ms"));

        let legacy = legacy_field(&out);
        assert_eq!(legacy["event"], "publish_succeeded");
        assert_eq!(legacy["duration_ms"], 4321);
    }

    #[test]
    fn to_micro_payload_publish_failed_marks_failure_and_carries_error() {
        let event = WebhookEvent::PublishFailed {
            plan_id: "plan-9".to_string(),
            package_name: "bad-crate".to_string(),
            package_version: "0.1.0".to_string(),
            error_class: "permanent".to_string(),
            message: "registry rejected publish".to_string(),
        };
        let out = to_micro_payload(&payload(event));

        assert_eq!(out.title.as_deref(), Some("Publish Failed"));
        assert!(!out.success, "PublishFailed must set success=false");
        assert_eq!(out.package.as_deref(), Some("bad-crate"));
        assert_eq!(out.version.as_deref(), Some("0.1.0"));
        assert_eq!(out.error.as_deref(), Some("registry rejected publish"));
        assert!(out.message.contains("bad-crate"));
        assert!(out.message.contains("0.1.0"));
        assert!(out.message.contains("permanent"));
        assert!(out.message.contains("registry rejected publish"));

        let legacy = legacy_field(&out);
        assert_eq!(legacy["event"], "publish_failed");
        assert_eq!(legacy["error_class"], "permanent");
    }

    #[test]
    fn to_micro_payload_publish_completed_success_when_no_failures() {
        let event = WebhookEvent::PublishCompleted {
            plan_id: "plan-1".to_string(),
            total_packages: 5,
            success_count: 5,
            failure_count: 0,
            skipped_count: 0,
            result: "success".to_string(),
        };
        let out = to_micro_payload(&payload(event));

        assert_eq!(out.title.as_deref(), Some("Publish Completed"));
        assert!(
            out.success,
            "completed with zero failures should be success=true"
        );
        assert!(out.package.is_none());
        assert!(out.version.is_none());
        assert!(out.registry.is_none());
        assert!(out.error.is_none());
        assert!(out.message.contains("5/5"));
        assert!(out.message.contains("plan-1"));

        let legacy = legacy_field(&out);
        assert_eq!(legacy["event"], "publish_completed");
        assert_eq!(legacy["total_packages"], 5);
        assert_eq!(legacy["success_count"], 5);
        assert_eq!(legacy["failure_count"], 0);
        assert_eq!(legacy["skipped_count"], 0);
        assert_eq!(legacy["result"], "success");
    }

    #[test]
    fn to_micro_payload_publish_completed_failure_when_any_failure() {
        let event = WebhookEvent::PublishCompleted {
            plan_id: "plan-1".to_string(),
            total_packages: 5,
            success_count: 3,
            failure_count: 2,
            skipped_count: 0,
            result: "failed".to_string(),
        };
        let out = to_micro_payload(&payload(event));

        assert!(
            !out.success,
            "any non-zero failure_count must set success=false"
        );
        let legacy = legacy_field(&out);
        assert_eq!(legacy["failure_count"], 2);
        assert_eq!(legacy["result"], "failed");
    }

    // ---- WebhookEvent serde tag/round-trip ----

    #[test]
    fn webhook_event_round_trips_through_serde_json() {
        let original = WebhookEvent::PublishSucceeded {
            plan_id: "plan-1".to_string(),
            package_name: "crate-a".to_string(),
            package_version: "0.1.0".to_string(),
            duration_ms: 100,
        };

        let json = serde_json::to_string(&original).expect("serialize");
        // serde(tag = "event", rename_all = "snake_case") -> tag is "publish_succeeded"
        assert!(
            json.contains("\"event\":\"publish_succeeded\""),
            "tag missing in serialized form: {json}"
        );

        let parsed: WebhookEvent = serde_json::from_str(&json).expect("deserialize");
        match parsed {
            WebhookEvent::PublishSucceeded {
                plan_id,
                package_name,
                package_version,
                duration_ms,
            } => {
                assert_eq!(plan_id, "plan-1");
                assert_eq!(package_name, "crate-a");
                assert_eq!(package_version, "0.1.0");
                assert_eq!(duration_ms, 100);
            }
            other => panic!("unexpected variant after round-trip: {other:?}"),
        }
    }

    #[test]
    fn webhook_event_failed_variant_tag_is_snake_case() {
        let event = WebhookEvent::PublishFailed {
            plan_id: "p".to_string(),
            package_name: "c".to_string(),
            package_version: "0.0.0".to_string(),
            error_class: "x".to_string(),
            message: "m".to_string(),
        };

        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"event\":\"publish_failed\""));
    }

    #[test]
    fn webhook_event_started_and_completed_variants_round_trip() {
        for event in [
            WebhookEvent::PublishStarted {
                plan_id: "p".to_string(),
                package_count: 1,
                registry: "r".to_string(),
            },
            WebhookEvent::PublishCompleted {
                plan_id: "p".to_string(),
                total_packages: 1,
                success_count: 1,
                failure_count: 0,
                skipped_count: 0,
                result: "success".to_string(),
            },
        ] {
            let json = serde_json::to_string(&event).expect("serialize");
            let parsed: WebhookEvent = serde_json::from_str(&json).expect("deserialize");
            // round-trip should produce equivalent serialization
            let json2 = serde_json::to_string(&parsed).expect("re-serialize");
            assert_eq!(json, json2);
        }
    }
}
