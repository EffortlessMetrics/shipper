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

    fn cfg(url: &str) -> WebhookConfig {
        WebhookConfig {
            url: url.to_string(),
            ..WebhookConfig::default()
        }
    }

    // --- WebhookClient::new ---

    fn new_err(config: &WebhookConfig) -> anyhow::Error {
        // WebhookClient doesn't impl Debug, so we can't use expect_err.
        match WebhookClient::new(config) {
            Ok(_) => panic!("expected an error from WebhookClient::new"),
            Err(e) => e,
        }
    }

    #[test]
    fn webhook_client_new_rejects_empty_url() {
        let err = new_err(&cfg(""));
        let msg = format!("{err:#}");
        assert!(msg.contains("webhook URL is required"), "got: {msg}");
    }

    #[test]
    fn webhook_client_new_rejects_whitespace_url() {
        // Trim happens before length check.
        let err = new_err(&cfg("   \t\n"));
        let msg = format!("{err:#}");
        assert!(msg.contains("webhook URL is required"), "got: {msg}");
    }

    #[test]
    fn webhook_client_new_accepts_nonempty_url() {
        let client = WebhookClient::new(&cfg("https://example.invalid/hook"))
            .expect("non-empty URL accepted");
        // We didn't expose `config` publicly, so just confirm the value
        // by going through send_event-free smoke (clone derives).
        let _cloned = client.clone();
    }

    // --- maybe_send_event ---

    #[test]
    fn maybe_send_event_is_noop_for_empty_url() {
        // Should NOT panic, spawn a thread, or send anything.
        let config = cfg("");
        maybe_send_event(
            &config,
            WebhookEvent::PublishStarted {
                plan_id: "p".into(),
                package_count: 0,
                registry: "crates-io".into(),
            },
        );
    }

    #[test]
    fn maybe_send_event_is_noop_for_whitespace_url() {
        let config = cfg("   ");
        maybe_send_event(
            &config,
            WebhookEvent::PublishCompleted {
                plan_id: "p".into(),
                total_packages: 0,
                success_count: 0,
                failure_count: 0,
                skipped_count: 0,
                result: "ok".into(),
            },
        );
    }

    // --- WebhookEvent serialization round-trips ---

    fn round_trip(ev: &WebhookEvent) -> WebhookEvent {
        let json = serde_json::to_string(ev).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn publish_started_round_trips_and_tags_event() {
        let ev = WebhookEvent::PublishStarted {
            plan_id: "plan-abc".into(),
            package_count: 3,
            registry: "crates-io".into(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        // serde tag = "event", rename_all = "snake_case"
        assert!(json.contains(r#""event":"publish_started""#), "json={json}");
        assert!(json.contains(r#""plan_id":"plan-abc""#));

        match round_trip(&ev) {
            WebhookEvent::PublishStarted {
                plan_id,
                package_count,
                registry,
            } => {
                assert_eq!(plan_id, "plan-abc");
                assert_eq!(package_count, 3);
                assert_eq!(registry, "crates-io");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn publish_succeeded_round_trips() {
        let ev = WebhookEvent::PublishSucceeded {
            plan_id: "p".into(),
            package_name: "foo".into(),
            package_version: "1.2.3".into(),
            duration_ms: 250,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""event":"publish_succeeded""#));
        match round_trip(&ev) {
            WebhookEvent::PublishSucceeded {
                package_name,
                duration_ms,
                ..
            } => {
                assert_eq!(package_name, "foo");
                assert_eq!(duration_ms, 250);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn publish_failed_round_trips() {
        let ev = WebhookEvent::PublishFailed {
            plan_id: "p".into(),
            package_name: "foo".into(),
            package_version: "1.0.0".into(),
            error_class: "permanent".into(),
            message: "denied".into(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""event":"publish_failed""#));
        match round_trip(&ev) {
            WebhookEvent::PublishFailed {
                error_class,
                message,
                ..
            } => {
                assert_eq!(error_class, "permanent");
                assert_eq!(message, "denied");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn publish_completed_round_trips() {
        let ev = WebhookEvent::PublishCompleted {
            plan_id: "p".into(),
            total_packages: 5,
            success_count: 4,
            failure_count: 1,
            skipped_count: 0,
            result: "partial_failure".into(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""event":"publish_completed""#));
        match round_trip(&ev) {
            WebhookEvent::PublishCompleted {
                total_packages,
                success_count,
                failure_count,
                result,
                ..
            } => {
                assert_eq!(total_packages, 5);
                assert_eq!(success_count, 4);
                assert_eq!(failure_count, 1);
                assert_eq!(result, "partial_failure");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    // --- WebhookPayload round-trip (uses Utc::now()) ---

    #[test]
    fn webhook_payload_serializes_with_timestamp_and_event() {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event: WebhookEvent::PublishStarted {
                plan_id: "p".into(),
                package_count: 1,
                registry: "crates-io".into(),
            },
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(json.contains("timestamp"));
        assert!(json.contains(r#""event":"publish_started""#));
    }

    // --- to_micro_payload converters: one assertion per variant ---

    fn extra_legacy(payload: &shipper_webhook::WebhookPayload) -> &serde_json::Value {
        payload
            .extra
            .get("legacy")
            .expect("`legacy` extra field present")
    }

    #[test]
    fn to_micro_payload_publish_started_maps_fields() {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event: WebhookEvent::PublishStarted {
                plan_id: "plan-1".into(),
                package_count: 4,
                registry: "crates-io".into(),
            },
        };
        let micro = to_micro_payload(&payload);
        assert!(micro.success, "publish_started is reported as success");
        assert_eq!(micro.title.as_deref(), Some("Publish Started"));
        assert!(micro.message.contains("plan-1"));
        assert!(micro.message.contains("4 packages"));
        assert!(micro.message.contains("crates-io"));
        assert_eq!(micro.registry.as_deref(), Some("crates-io"));
        assert!(micro.package.is_none());
        assert!(micro.version.is_none());
        assert!(micro.error.is_none());

        let extra = extra_legacy(&micro);
        assert_eq!(extra["event"], "publish_started");
        assert_eq!(extra["plan_id"], "plan-1");
        assert_eq!(extra["package_count"], 4);
        assert_eq!(extra["registry"], "crates-io");
    }

    #[test]
    fn to_micro_payload_publish_succeeded_maps_fields() {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event: WebhookEvent::PublishSucceeded {
                plan_id: "plan-1".into(),
                package_name: "foo".into(),
                package_version: "0.1.0".into(),
                duration_ms: 1234,
            },
        };
        let micro = to_micro_payload(&payload);
        assert!(micro.success);
        assert_eq!(micro.title.as_deref(), Some("Publish Succeeded"));
        assert!(micro.message.contains("foo"));
        assert!(micro.message.contains("0.1.0"));
        assert!(micro.message.contains("1234ms"));
        assert_eq!(micro.package.as_deref(), Some("foo"));
        assert_eq!(micro.version.as_deref(), Some("0.1.0"));
        assert!(micro.registry.is_none());
        assert!(micro.error.is_none());

        let extra = extra_legacy(&micro);
        assert_eq!(extra["event"], "publish_succeeded");
        assert_eq!(extra["duration_ms"], 1234);
    }

    #[test]
    fn to_micro_payload_publish_failed_sets_success_false_and_error() {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event: WebhookEvent::PublishFailed {
                plan_id: "plan-9".into(),
                package_name: "bar".into(),
                package_version: "2.0.0".into(),
                error_class: "permanent".into(),
                message: "version already exists".into(),
            },
        };
        let micro = to_micro_payload(&payload);
        assert!(!micro.success, "failures must report success=false");
        assert_eq!(micro.title.as_deref(), Some("Publish Failed"));
        assert!(micro.message.contains("bar"));
        assert!(micro.message.contains("permanent"));
        assert_eq!(micro.package.as_deref(), Some("bar"));
        assert_eq!(micro.version.as_deref(), Some("2.0.0"));
        assert_eq!(micro.error.as_deref(), Some("version already exists"));

        let extra = extra_legacy(&micro);
        assert_eq!(extra["event"], "publish_failed");
        assert_eq!(extra["error_class"], "permanent");
    }

    #[test]
    fn to_micro_payload_publish_completed_success_when_no_failures() {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event: WebhookEvent::PublishCompleted {
                plan_id: "p".into(),
                total_packages: 3,
                success_count: 3,
                failure_count: 0,
                skipped_count: 0,
                result: "success".into(),
            },
        };
        let micro = to_micro_payload(&payload);
        assert!(
            micro.success,
            "all-succeeded completion must report success=true"
        );
        assert_eq!(micro.title.as_deref(), Some("Publish Completed"));
        assert!(micro.message.contains("3/3"));
        assert!(micro.message.contains("success"));
        assert!(micro.package.is_none());

        let extra = extra_legacy(&micro);
        assert_eq!(extra["event"], "publish_completed");
        assert_eq!(extra["total_packages"], 3);
        assert_eq!(extra["success_count"], 3);
        assert_eq!(extra["failure_count"], 0);
        assert_eq!(extra["skipped_count"], 0);
        assert_eq!(extra["result"], "success");
    }

    #[test]
    fn to_micro_payload_publish_completed_failure_when_any_failures() {
        let payload = WebhookPayload {
            timestamp: Utc::now(),
            event: WebhookEvent::PublishCompleted {
                plan_id: "p".into(),
                total_packages: 3,
                success_count: 1,
                failure_count: 2,
                skipped_count: 0,
                result: "partial_failure".into(),
            },
        };
        let micro = to_micro_payload(&payload);
        assert!(
            !micro.success,
            "any-failure completion must report success=false"
        );
        let extra = extra_legacy(&micro);
        assert_eq!(extra["failure_count"], 2);
        assert_eq!(extra["result"], "partial_failure");
    }
}
