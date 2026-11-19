//! Webhook contract values.
//!
//! These are the pure configuration values that describe *where* and *how* a
//! webhook notification is addressed. They carry no delivery behavior: HTTP
//! clients, signing, and payload rendering live in `shipper-webhook`, which
//! consumes the definitions here and re-exports them for compatibility.
//!
//! Keeping the values in the contract crate is what lets `shipper-types` stay
//! free of an HTTP/TLS dependency while still describing the runtime options
//! that reference a webhook (see [`crate::RuntimeOptions::webhook`]).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Webhook type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebhookType {
    /// Generic webhook (POST JSON)
    #[default]
    Generic,
    /// Slack incoming webhook
    Slack,
    /// Discord webhook
    Discord,
}

/// Webhook configuration
///
/// `Debug` is implemented manually so signing secrets and credential-bearing
/// webhook URLs are not copied into diagnostics, error chains, or evidence
/// that formats configuration values.
#[derive(Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Webhook URL
    pub url: String,
    /// Type of webhook
    #[serde(default)]
    pub webhook_type: WebhookType,
    /// Optional secret for payload signing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

/// Default delivery timeout, in seconds.
///
/// This is a persisted contract value, not just an internal constant: it is the
/// `serde` default for [`WebhookConfig::timeout_secs`], so a configuration file
/// that omits the field deserializes to it. Changing it changes the meaning of
/// every stored webhook configuration that relies on the default.
fn default_timeout() -> u64 {
    30
}

/// Placeholder written in place of a credential-bearing value by
/// [`WebhookConfig`]'s manual [`fmt::Debug`] implementation.
const REDACTED_SECRET: &str = "<redacted>";

impl fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let safe_url = if self.url.is_empty() {
            ""
        } else {
            REDACTED_SECRET
        };
        f.debug_struct("WebhookConfig")
            .field("url", &safe_url)
            .field("webhook_type", &self.webhook_type)
            .field("secret", &self.secret.as_ref().map(|_| REDACTED_SECRET))
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            webhook_type: WebhookType::default(),
            secret: None,
            timeout_secs: default_timeout(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled_generic_with_thirty_second_timeout() {
        let config = WebhookConfig::default();
        assert_eq!(config.url, "");
        assert_eq!(config.webhook_type, WebhookType::Generic);
        assert_eq!(config.secret, None);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn webhook_type_serializes_with_pascal_case_variant_names() {
        // The wire form is unqualified variant names, not snake_case. Persisted
        // configuration depends on it, so pin it here rather than inferring it
        // from the derive.
        for (variant, expected) in [
            (WebhookType::Generic, "\"Generic\""),
            (WebhookType::Slack, "\"Slack\""),
            (WebhookType::Discord, "\"Discord\""),
        ] {
            assert_eq!(
                serde_json::to_string(&variant).expect("serialize"),
                expected
            );
        }
    }

    #[test]
    fn absent_optional_fields_fall_back_to_defaults() {
        let config: WebhookConfig =
            serde_json::from_str(r#"{"url":"https://example.test/hook"}"#).expect("deserialize");
        assert_eq!(config.webhook_type, WebhookType::Generic);
        assert_eq!(config.secret, None);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn absent_secret_is_omitted_from_the_serialized_form() {
        let json = serde_json::to_value(WebhookConfig {
            url: "https://example.test/hook".to_string(),
            ..Default::default()
        })
        .expect("serialize");
        assert!(
            json.get("secret").is_none(),
            "absent secret must not be written as null: {json}"
        );
    }

    #[test]
    fn debug_redacts_the_url_and_the_secret() {
        let config = WebhookConfig {
            url: "https://hooks.example.test/T000/B000/xoxb-sentinel".to_string(),
            webhook_type: WebhookType::Slack,
            secret: Some("shipper-webhook-secret-sentinel".to_string()),
            timeout_secs: 5,
        };

        let rendered = format!("{config:?}");

        assert!(
            !rendered.contains("xoxb-sentinel"),
            "url leaked: {rendered}"
        );
        assert!(
            !rendered.contains("shipper-webhook-secret-sentinel"),
            "secret leaked: {rendered}"
        );
        assert!(rendered.contains("Slack"), "shape lost: {rendered}");
        assert!(rendered.contains('5'), "timeout lost: {rendered}");
    }

    #[test]
    fn debug_of_an_empty_url_stays_empty_rather_than_claiming_a_redaction() {
        let rendered = format!("{:?}", WebhookConfig::default());
        assert!(
            !rendered.contains(REDACTED_SECRET),
            "an unset webhook must not look configured-and-hidden: {rendered}"
        );
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let config = WebhookConfig {
            url: "https://example.test/hook".to_string(),
            webhook_type: WebhookType::Discord,
            secret: Some("s".to_string()),
            timeout_secs: 11,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let restored: WebhookConfig = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.url, config.url);
        assert_eq!(restored.webhook_type, config.webhook_type);
        assert_eq!(restored.secret, config.secret);
        assert_eq!(restored.timeout_secs, config.timeout_secs);
    }
}
