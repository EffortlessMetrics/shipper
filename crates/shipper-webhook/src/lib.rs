//! Webhook notifications for shipper.
//!
//! This crate provides webhook notification support for publish events,
//! supporting Slack, Discord, and generic webhooks.
//!
//! # Example
//!
//! ```ignore
//! use shipper_webhook::{WebhookConfig, send_webhook, WebhookPayload};
//!
//! let config = WebhookConfig {
//!     url: "https://hooks.slack.com/services/...".to_string(),
//!     webhook_type: WebhookType::Slack,
//! };
//!
//! let payload = WebhookPayload {
//!     message: "Published my-crate@1.0.0".to_string(),
//!     ..Default::default()
//! };
//!
//! send_webhook(&config, &payload).expect("send");
//! ```

use std::time::Duration;

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn default_timeout() -> u64 {
    30
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

/// Webhook payload
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Main message
    pub message: String,
    /// Optional title
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Success status
    pub success: bool,
    /// Package name (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Version (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Registry (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Additional fields
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Send a webhook notification
pub fn send_webhook(config: &WebhookConfig, payload: &WebhookPayload) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .build()
        .context("failed to create HTTP client")?;

    let body = match config.webhook_type {
        WebhookType::Generic => serde_json::to_string(payload)?,
        WebhookType::Slack => slack_payload(payload)?,
        WebhookType::Discord => discord_payload(payload)?,
    };

    let signature = config
        .secret
        .as_deref()
        .filter(|secret| !secret.trim().is_empty())
        .map(|secret| webhook_signature(secret, &body))
        .transpose()?;

    let mut request = client
        .post(&config.url)
        .header("Content-Type", "application/json")
        .body(body);

    if let Some(signature) = signature {
        request = request.header("X-Hub-Signature-256", signature);
    }

    let response = request.send().context("failed to send webhook request")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "webhook request failed with status {}: {}",
            response.status(),
            response.text().unwrap_or_default()
        ));
    }

    Ok(())
}

/// Send a webhook notification asynchronously
pub async fn send_webhook_async(config: &WebhookConfig, payload: &WebhookPayload) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .build()
        .context("failed to create HTTP client")?;

    let body = match config.webhook_type {
        WebhookType::Generic => serde_json::to_string(payload)?,
        WebhookType::Slack => slack_payload(payload)?,
        WebhookType::Discord => discord_payload(payload)?,
    };

    let signature = config
        .secret
        .as_deref()
        .filter(|secret| !secret.trim().is_empty())
        .map(|secret| webhook_signature(secret, &body))
        .transpose()?;

    let mut request = client
        .post(&config.url)
        .header("Content-Type", "application/json")
        .body(body);

    if let Some(signature) = signature {
        request = request.header("X-Hub-Signature-256", signature);
    }

    let response = request
        .send()
        .await
        .context("failed to send webhook request")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "webhook request failed with status {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ));
    }

    Ok(())
}

fn webhook_signature(secret: &str, body: &str) -> Result<String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).context("invalid webhook secret")?;
    mac.update(body.as_bytes());
    let digest = mac.finalize().into_bytes();
    Ok(format!("sha256={}", hex_encode(&digest)))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

/// Format payload for Slack
fn slack_payload(payload: &WebhookPayload) -> Result<String> {
    let color = if payload.success { "good" } else { "danger" };

    let mut fields = vec![];

    if let Some(package) = &payload.package {
        fields.push(json!({
            "title": "Package",
            "value": package,
            "short": true
        }));
    }

    if let Some(version) = &payload.version {
        fields.push(json!({
            "title": "Version",
            "value": version,
            "short": true
        }));
    }

    if let Some(registry) = &payload.registry {
        fields.push(json!({
            "title": "Registry",
            "value": registry,
            "short": true
        }));
    }

    if let Some(error) = &payload.error {
        fields.push(json!({
            "title": "Error",
            "value": error,
            "short": false
        }));
    }

    let slack_json = json!({
        "attachments": [{
            "color": color,
            "title": payload.title.as_ref().unwrap_or(&"Shipper Notification".to_string()),
            "text": payload.message,
            "fields": fields
        }]
    });

    Ok(serde_json::to_string(&slack_json)?)
}

/// Format payload for Discord
fn discord_payload(payload: &WebhookPayload) -> Result<String> {
    let color = if payload.success {
        65280_u32
    } else {
        16711680_u32
    };

    let mut fields = vec![];

    if let Some(package) = &payload.package {
        fields.push(json!({
            "name": "Package",
            "value": package,
            "inline": true
        }));
    }

    if let Some(version) = &payload.version {
        fields.push(json!({
            "name": "Version",
            "value": version,
            "inline": true
        }));
    }

    if let Some(registry) = &payload.registry {
        fields.push(json!({
            "name": "Registry",
            "value": registry,
            "inline": true
        }));
    }

    if let Some(error) = &payload.error {
        fields.push(json!({
            "name": "Error",
            "value": error,
            "inline": false
        }));
    }

    let discord_json = json!({
        "embeds": [{
            "title": payload.title.as_ref().unwrap_or(&"Shipper Notification".to_string()),
            "description": payload.message,
            "color": color,
            "fields": fields
        }]
    });

    Ok(serde_json::to_string(&discord_json)?)
}

/// Create a success payload for a published package
pub fn publish_success_payload(package: &str, version: &str, registry: &str) -> WebhookPayload {
    WebhookPayload {
        message: format!("Successfully published {}@{}", package, version),
        title: Some("Package Published".to_string()),
        success: true,
        package: Some(package.to_string()),
        version: Some(version.to_string()),
        registry: Some(registry.to_string()),
        ..Default::default()
    }
}

/// Create a failure payload for a failed publish
pub fn publish_failure_payload(package: &str, version: &str, error: &str) -> WebhookPayload {
    WebhookPayload {
        message: format!("Failed to publish {}@{}", package, version),
        title: Some("Publish Failed".to_string()),
        success: false,
        package: Some(package.to_string()),
        version: Some(version.to_string()),
        error: Some(error.to_string()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_type_default() {
        let wt = WebhookType::default();
        assert_eq!(wt, WebhookType::Generic);
    }

    #[test]
    fn webhook_config_default() {
        let config = WebhookConfig::default();
        assert!(config.url.is_empty());
        assert_eq!(config.webhook_type, WebhookType::Generic);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn webhook_payload_default() {
        let payload = WebhookPayload::default();
        assert!(payload.message.is_empty());
        assert!(!payload.success);
    }

    #[test]
    fn publish_success_payload_works() {
        let payload = publish_success_payload("my-crate", "1.0.0", "crates-io");
        assert!(payload.success);
        assert_eq!(payload.package, Some("my-crate".to_string()));
        assert_eq!(payload.version, Some("1.0.0".to_string()));
        assert!(payload.message.contains("Successfully"));
    }

    #[test]
    fn publish_failure_payload_works() {
        let payload = publish_failure_payload("my-crate", "1.0.0", "network error");
        assert!(!payload.success);
        assert_eq!(payload.error, Some("network error".to_string()));
        assert!(payload.message.contains("Failed"));
    }

    #[test]
    fn slack_payload_format() {
        let payload = publish_success_payload("test", "1.0.0", "crates-io");
        let json = slack_payload(&payload).expect("format");

        assert!(json.contains("\"attachments\""));
        assert!(json.contains("\"color\":\"good\""));
        assert!(json.contains("test"));
    }

    #[test]
    fn discord_payload_format() {
        let payload = publish_success_payload("test", "1.0.0", "crates-io");
        let json = discord_payload(&payload).expect("format");

        assert!(json.contains("\"embeds\""));
        assert!(json.contains("\"color\":65280"));
        assert!(json.contains("test"));
    }

    #[test]
    fn webhook_config_serialization() {
        let config = WebhookConfig {
            url: "https://example.com/webhook".to_string(),
            webhook_type: WebhookType::Slack,
            secret: None,
            timeout_secs: 60,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("\"url\""));
        assert!(json.contains("\"webhook_type\":\"Slack\""));
    }

    #[test]
    fn webhook_payload_serialization() {
        let payload = WebhookPayload {
            message: "test message".to_string(),
            success: true,
            ..Default::default()
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(json.contains("\"message\":\"test message\""));
        assert!(json.contains("\"success\":true"));
    }

    #[test]
    fn slack_payload_failure_color() {
        let payload = publish_failure_payload("test", "1.0.0", "error");
        let json = slack_payload(&payload).expect("format");
        assert!(json.contains("\"color\":\"danger\""));
    }

    #[test]
    fn discord_payload_failure_color() {
        let payload = publish_failure_payload("test", "1.0.0", "error");
        let json = discord_payload(&payload).expect("format");
        assert!(json.contains("\"color\":16711680"));
    }

    #[test]
    fn webhook_signature_matches_known_hmac_sha256() {
        let signature = webhook_signature("secret", "hello").expect("signature");
        assert_eq!(
            signature,
            "sha256=88aab3ede8d3adf94d26ab90d3bafd4a2083070c3bcce9c014ee04a443847c0b"
        );
    }

    // --- Payload construction ---

    #[test]
    fn publish_success_payload_contains_registry() {
        let payload = publish_success_payload("foo", "2.0.0", "crates-io");
        assert_eq!(payload.registry, Some("crates-io".to_string()));
        assert!(payload.error.is_none());
        assert_eq!(payload.title, Some("Package Published".to_string()));
    }

    #[test]
    fn publish_failure_payload_has_no_registry() {
        let payload = publish_failure_payload("foo", "0.1.0", "timeout");
        assert!(payload.registry.is_none());
        assert_eq!(payload.error, Some("timeout".to_string()));
        assert_eq!(payload.title, Some("Publish Failed".to_string()));
    }

    #[test]
    fn payload_with_all_fields() {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("ci".to_string(), serde_json::json!("github-actions"));

        let payload = WebhookPayload {
            message: "msg".to_string(),
            title: Some("title".to_string()),
            success: true,
            package: Some("pkg".to_string()),
            version: Some("1.0.0".to_string()),
            registry: Some("crates-io".to_string()),
            error: None,
            extra,
        };

        assert_eq!(payload.message, "msg");
        assert_eq!(payload.extra.get("ci").unwrap(), "github-actions");
    }

    #[test]
    fn payload_extra_fields_flatten_in_json() {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("run_id".to_string(), serde_json::json!(42));

        let payload = WebhookPayload {
            message: "m".to_string(),
            extra,
            ..Default::default()
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"run_id\":42"));
        // Flattened means no "extra" key wrapper
        assert!(!json.contains("\"extra\""));
    }

    // --- Serialization round-trips ---

    #[test]
    fn webhook_payload_roundtrip() {
        let payload = publish_success_payload("my-crate", "3.0.0", "crates-io");
        let json = serde_json::to_string(&payload).unwrap();
        let deserialized: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message, payload.message);
        assert_eq!(deserialized.success, payload.success);
        assert_eq!(deserialized.package, payload.package);
        assert_eq!(deserialized.version, payload.version);
        assert_eq!(deserialized.registry, payload.registry);
    }

    #[test]
    fn webhook_config_roundtrip() {
        let config = WebhookConfig {
            url: "https://example.com/hook".to_string(),
            webhook_type: WebhookType::Discord,
            secret: Some("s3cret".to_string()),
            timeout_secs: 10,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WebhookConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.url, config.url);
        assert_eq!(deserialized.webhook_type, WebhookType::Discord);
        assert_eq!(deserialized.secret, Some("s3cret".to_string()));
        assert_eq!(deserialized.timeout_secs, 10);
    }

    #[test]
    fn config_deserialization_defaults() {
        // Only url is required; other fields should pick up defaults
        let json = r#"{"url":"https://x.com"}"#;
        let config: WebhookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.webhook_type, WebhookType::Generic);
        assert_eq!(config.timeout_secs, 30);
        assert!(config.secret.is_none());
    }

    #[test]
    fn config_secret_omitted_when_none() {
        let config = WebhookConfig {
            url: "https://x.com".to_string(),
            secret: None,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("secret"));
    }

    #[test]
    fn payload_optional_fields_omitted_when_none() {
        let payload = WebhookPayload {
            message: "hi".to_string(),
            success: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("\"title\""));
        assert!(!json.contains("\"package\""));
        assert!(!json.contains("\"version\""));
        assert!(!json.contains("\"registry\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn webhook_type_all_variants_serialize() {
        let generic = serde_json::to_string(&WebhookType::Generic).unwrap();
        let slack = serde_json::to_string(&WebhookType::Slack).unwrap();
        let discord = serde_json::to_string(&WebhookType::Discord).unwrap();
        assert_eq!(generic, "\"Generic\"");
        assert_eq!(slack, "\"Slack\"");
        assert_eq!(discord, "\"Discord\"");
    }

    #[test]
    fn webhook_type_all_variants_deserialize() {
        let g: WebhookType = serde_json::from_str("\"Generic\"").unwrap();
        let s: WebhookType = serde_json::from_str("\"Slack\"").unwrap();
        let d: WebhookType = serde_json::from_str("\"Discord\"").unwrap();
        assert_eq!(g, WebhookType::Generic);
        assert_eq!(s, WebhookType::Slack);
        assert_eq!(d, WebhookType::Discord);
    }

    // --- Slack formatting ---

    #[test]
    fn slack_payload_with_all_fields() {
        let payload = WebhookPayload {
            message: "deployed".to_string(),
            title: Some("Deploy".to_string()),
            success: false,
            package: Some("pkg".to_string()),
            version: Some("0.1.0".to_string()),
            registry: Some("reg".to_string()),
            error: Some("oops".to_string()),
            ..Default::default()
        };
        let json = slack_payload(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let attachment = &parsed["attachments"][0];
        assert_eq!(attachment["color"], "danger");
        assert_eq!(attachment["title"], "Deploy");
        assert_eq!(attachment["text"], "deployed");

        let fields = attachment["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4);
    }

    #[test]
    fn slack_payload_no_optional_fields() {
        let payload = WebhookPayload {
            message: "hello".to_string(),
            success: true,
            ..Default::default()
        };
        let json = slack_payload(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let attachment = &parsed["attachments"][0];
        assert_eq!(attachment["color"], "good");
        // Default title
        assert_eq!(attachment["title"], "Shipper Notification");
        let fields = attachment["fields"].as_array().unwrap();
        assert!(fields.is_empty());
    }

    // --- Discord formatting ---

    #[test]
    fn discord_payload_with_all_fields() {
        let payload = WebhookPayload {
            message: "done".to_string(),
            title: Some("Release".to_string()),
            success: true,
            package: Some("crate-a".to_string()),
            version: Some("2.0.0".to_string()),
            registry: Some("crates-io".to_string()),
            error: Some("warn".to_string()),
            ..Default::default()
        };
        let json = discord_payload(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let embed = &parsed["embeds"][0];
        assert_eq!(embed["color"], 65280);
        assert_eq!(embed["title"], "Release");
        assert_eq!(embed["description"], "done");

        let fields = embed["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4);
    }

    #[test]
    fn discord_payload_no_optional_fields() {
        let payload = WebhookPayload {
            message: "hi".to_string(),
            success: false,
            ..Default::default()
        };
        let json = discord_payload(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let embed = &parsed["embeds"][0];
        assert_eq!(embed["color"], 16711680);
        assert_eq!(embed["title"], "Shipper Notification");
        let fields = embed["fields"].as_array().unwrap();
        assert!(fields.is_empty());
    }

    // --- Signature / HMAC ---

    #[test]
    fn signature_prefix() {
        let sig = webhook_signature("key", "data").unwrap();
        assert!(sig.starts_with("sha256="));
    }

    #[test]
    fn different_secrets_produce_different_signatures() {
        let s1 = webhook_signature("secret-a", "body").unwrap();
        let s2 = webhook_signature("secret-b", "body").unwrap();
        assert_ne!(s1, s2);
    }

    #[test]
    fn different_bodies_produce_different_signatures() {
        let s1 = webhook_signature("key", "body-a").unwrap();
        let s2 = webhook_signature("key", "body-b").unwrap();
        assert_ne!(s1, s2);
    }

    #[test]
    fn signature_on_empty_body() {
        let sig = webhook_signature("secret", "").unwrap();
        assert!(sig.starts_with("sha256="));
        assert!(sig.len() > "sha256=".len());
    }

    #[test]
    fn hex_encode_empty() {
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn hex_encode_known() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
    }

    #[test]
    fn hex_encode_all_single_digits() {
        assert_eq!(hex_encode(&[0x0a, 0x0b, 0x0c]), "0a0b0c");
    }

    // --- Error handling (send_webhook against unreachable endpoints) ---

    #[test]
    fn send_webhook_invalid_url_returns_error() {
        let config = WebhookConfig {
            url: "not-a-url".to_string(),
            timeout_secs: 1,
            ..Default::default()
        };
        let payload = WebhookPayload {
            message: "test".to_string(),
            ..Default::default()
        };
        let result = send_webhook(&config, &payload);
        assert!(result.is_err());
    }

    #[test]
    fn send_webhook_connection_refused_returns_error() {
        // Port 1 is almost certainly not listening
        let config = WebhookConfig {
            url: "http://127.0.0.1:1/webhook".to_string(),
            timeout_secs: 1,
            ..Default::default()
        };
        let payload = WebhookPayload {
            message: "test".to_string(),
            ..Default::default()
        };
        let result = send_webhook(&config, &payload);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_webhook_async_connection_refused_returns_error() {
        let config = WebhookConfig {
            url: "http://127.0.0.1:1/webhook".to_string(),
            timeout_secs: 1,
            ..Default::default()
        };
        let payload = WebhookPayload {
            message: "test".to_string(),
            ..Default::default()
        };
        let result = send_webhook_async(&config, &payload).await;
        assert!(result.is_err());
    }

    // --- Mock server tests ---

    #[test]
    fn send_webhook_success_with_mock_server() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            webhook_type: WebhookType::Generic,
            timeout_secs: 5,
            secret: None,
        };
        let payload = publish_success_payload("mypkg", "1.0.0", "crates-io");

        let handle = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            assert_eq!(req.method(), &tiny_http::Method::Post);
            assert_eq!(req.url(), "/hook");
            // Should not have signature header when no secret
            assert!(req.headers().iter().all(|h| {
                !h.field
                    .as_str()
                    .as_str()
                    .eq_ignore_ascii_case("x-hub-signature-256")
            }));
            let response = tiny_http::Response::from_string("ok");
            req.respond(response).unwrap();
        });

        let result = send_webhook(&config, &payload);
        handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn send_webhook_with_signature_header() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/signed"),
            webhook_type: WebhookType::Generic,
            timeout_secs: 5,
            secret: Some("my-secret".to_string()),
        };
        let payload = WebhookPayload {
            message: "signed".to_string(),
            ..Default::default()
        };

        let handle = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            let sig_header = req
                .headers()
                .iter()
                .find(|h| {
                    h.field
                        .as_str()
                        .as_str()
                        .eq_ignore_ascii_case("x-hub-signature-256")
                })
                .expect("signature header missing");
            assert!(sig_header.value.as_str().starts_with("sha256="));
            let response = tiny_http::Response::from_string("ok");
            req.respond(response).unwrap();
        });

        let result = send_webhook(&config, &payload);
        handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn send_webhook_empty_secret_skips_signature() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            webhook_type: WebhookType::Generic,
            timeout_secs: 5,
            secret: Some("   ".to_string()), // whitespace-only
        };
        let payload = WebhookPayload {
            message: "test".to_string(),
            ..Default::default()
        };

        let handle = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            // Whitespace-only secret should NOT produce a signature
            assert!(req.headers().iter().all(|h| {
                !h.field
                    .as_str()
                    .as_str()
                    .eq_ignore_ascii_case("x-hub-signature-256")
            }));
            let response = tiny_http::Response::from_string("ok");
            req.respond(response).unwrap();
        });

        let result = send_webhook(&config, &payload);
        handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn send_webhook_server_error_returns_err() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/fail"),
            timeout_secs: 5,
            ..Default::default()
        };
        let payload = WebhookPayload {
            message: "test".to_string(),
            ..Default::default()
        };

        let handle = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            let response = tiny_http::Response::from_string("internal error")
                .with_status_code(tiny_http::StatusCode(500));
            req.respond(response).unwrap();
        });

        let result = send_webhook(&config, &payload);
        handle.join().unwrap();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("500"));
    }

    #[test]
    fn send_webhook_slack_format_to_server() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/slack"),
            webhook_type: WebhookType::Slack,
            timeout_secs: 5,
            secret: None,
        };
        let payload = publish_success_payload("crate-x", "0.1.0", "crates-io");

        let handle = std::thread::spawn(move || {
            let mut req = server.recv().unwrap();
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
            // Slack format has "attachments"
            assert!(parsed["attachments"].is_array());
            let response = tiny_http::Response::from_string("ok");
            req.respond(response).unwrap();
        });

        let result = send_webhook(&config, &payload);
        handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn send_webhook_discord_format_to_server() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/discord"),
            webhook_type: WebhookType::Discord,
            timeout_secs: 5,
            secret: None,
        };
        let payload = publish_failure_payload("crate-y", "0.2.0", "network error");

        let handle = std::thread::spawn(move || {
            let mut req = server.recv().unwrap();
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
            // Discord format has "embeds"
            assert!(parsed["embeds"].is_array());
            let response = tiny_http::Response::from_string("ok");
            req.respond(response).unwrap();
        });

        let result = send_webhook(&config, &payload);
        handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_webhook_async_success_with_mock_server() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/async-hook"),
            webhook_type: WebhookType::Generic,
            timeout_secs: 5,
            secret: None,
        };
        let payload = publish_success_payload("async-pkg", "1.0.0", "crates-io");

        let handle = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            let response = tiny_http::Response::from_string("ok");
            req.respond(response).unwrap();
        });

        let result = send_webhook_async(&config, &payload).await;
        handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_webhook_async_server_error() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();

        let config = WebhookConfig {
            url: format!("http://{addr}/fail"),
            timeout_secs: 5,
            ..Default::default()
        };
        let payload = WebhookPayload {
            message: "test".to_string(),
            ..Default::default()
        };

        let handle = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            let response = tiny_http::Response::from_string("bad")
                .with_status_code(tiny_http::StatusCode(403));
            req.respond(response).unwrap();
        });

        let result = send_webhook_async(&config, &payload).await;
        handle.join().unwrap();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("403"));
    }

    // --- Snapshot tests (insta) ---

    mod snapshot_tests {
        use super::*;

        // -- Webhook payload JSON structure for different event types --

        #[test]
        fn generic_success_payload_json() {
            let payload = publish_success_payload("my-crate", "1.2.3", "crates-io");
            let json: serde_json::Value = serde_json::to_value(&payload).unwrap();
            insta::assert_yaml_snapshot!("generic_success_payload", json);
        }

        #[test]
        fn generic_failure_payload_json() {
            let payload =
                publish_failure_payload("my-crate", "1.2.3", "timeout waiting for registry");
            let json: serde_json::Value = serde_json::to_value(&payload).unwrap();
            insta::assert_yaml_snapshot!("generic_failure_payload", json);
        }

        #[test]
        fn slack_success_payload_json() {
            let payload = publish_success_payload("my-crate", "1.2.3", "crates-io");
            let body = slack_payload(&payload).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            insta::assert_yaml_snapshot!("slack_success_payload", json);
        }

        #[test]
        fn slack_failure_payload_json() {
            let payload = publish_failure_payload("my-crate", "1.2.3", "network error");
            let body = slack_payload(&payload).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            insta::assert_yaml_snapshot!("slack_failure_payload", json);
        }

        #[test]
        fn discord_success_payload_json() {
            let payload = publish_success_payload("my-crate", "1.2.3", "crates-io");
            let body = discord_payload(&payload).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            insta::assert_yaml_snapshot!("discord_success_payload", json);
        }

        #[test]
        fn discord_failure_payload_json() {
            let payload = publish_failure_payload("my-crate", "1.2.3", "network error");
            let body = discord_payload(&payload).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            insta::assert_yaml_snapshot!("discord_failure_payload", json);
        }

        #[test]
        fn generic_minimal_payload_json() {
            let payload = WebhookPayload {
                message: "hello".to_string(),
                success: true,
                ..Default::default()
            };
            let json: serde_json::Value = serde_json::to_value(&payload).unwrap();
            insta::assert_yaml_snapshot!("generic_minimal_payload", json);
        }

        #[test]
        fn generic_payload_with_extra_fields() {
            let mut extra = std::collections::BTreeMap::new();
            extra.insert("ci".to_string(), serde_json::json!("github-actions"));
            extra.insert("run_id".to_string(), serde_json::json!(42));
            let payload = WebhookPayload {
                message: "deployed".to_string(),
                title: Some("Deploy".to_string()),
                success: true,
                package: Some("my-crate".to_string()),
                version: Some("1.0.0".to_string()),
                registry: Some("crates-io".to_string()),
                error: None,
                extra,
            };
            let json: serde_json::Value = serde_json::to_value(&payload).unwrap();
            insta::assert_yaml_snapshot!("generic_payload_with_extras", json);
        }

        // -- Webhook configuration serialization --

        #[test]
        fn config_generic_default() {
            let config = WebhookConfig {
                url: "https://example.com/webhook".to_string(),
                ..Default::default()
            };
            let json: serde_json::Value = serde_json::to_value(&config).unwrap();
            insta::assert_yaml_snapshot!("config_generic_default", json);
        }

        #[test]
        fn config_slack_with_secret() {
            let config = WebhookConfig {
                url: "https://hooks.slack.com/services/T00/B00/xxx".to_string(),
                webhook_type: WebhookType::Slack,
                secret: Some("s3cret-key".to_string()),
                timeout_secs: 10,
            };
            let json: serde_json::Value = serde_json::to_value(&config).unwrap();
            insta::assert_yaml_snapshot!("config_slack_with_secret", json);
        }

        #[test]
        fn config_discord_no_secret() {
            let config = WebhookConfig {
                url: "https://discord.com/api/webhooks/123/abc".to_string(),
                webhook_type: WebhookType::Discord,
                secret: None,
                timeout_secs: 60,
            };
            let json: serde_json::Value = serde_json::to_value(&config).unwrap();
            insta::assert_yaml_snapshot!("config_discord_no_secret", json);
        }

        // -- Error message formatting for delivery failures --

        #[test]
        fn error_webhook_status_500() {
            let err = anyhow::anyhow!(
                "webhook request failed with status 500 Internal Server Error: internal error"
            );
            insta::assert_snapshot!("error_status_500", err.to_string());
        }

        #[test]
        fn error_webhook_status_403() {
            let err = anyhow::anyhow!("webhook request failed with status 403 Forbidden: bad");
            insta::assert_snapshot!("error_status_403", err.to_string());
        }

        #[test]
        fn error_send_failure() {
            let inner =
                std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
            let err = anyhow::Error::new(inner).context("failed to send webhook request");
            insta::assert_snapshot!("error_send_failure", err.to_string());
        }

        // -- URL validation error messages --

        #[test]
        fn error_invalid_url() {
            let config = WebhookConfig {
                url: "not-a-url".to_string(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            let err = send_webhook(&config, &payload).unwrap_err();
            insta::assert_snapshot!("error_invalid_url", err.to_string());
        }

        #[test]
        fn error_connection_refused() {
            let config = WebhookConfig {
                url: "http://127.0.0.1:1/webhook".to_string(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            let err = send_webhook(&config, &payload).unwrap_err();
            insta::assert_snapshot!("error_connection_refused", err.to_string());
        }
    }

    // --- Property-based tests (proptest) ---

    mod prop {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating crate-like package names.
        fn package_name() -> impl Strategy<Value = String> {
            "[a-z][a-z0-9_-]{0,39}".prop_map(|s| s)
        }

        /// Strategy for generating semver-like versions.
        fn version_string() -> impl Strategy<Value = String> {
            (0u32..100, 0u32..100, 0u32..100).prop_map(|(ma, mi, pa)| format!("{ma}.{mi}.{pa}"))
        }

        /// Strategy for generating an arbitrary WebhookPayload.
        fn arb_payload() -> impl Strategy<Value = WebhookPayload> {
            (
                ".*",                                   // message
                proptest::option::of(".*"),             // title
                any::<bool>(),                          // success
                proptest::option::of(package_name()),   // package
                proptest::option::of(version_string()), // version
                proptest::option::of("[a-z-]{1,20}"),   // registry
                proptest::option::of(".*"),             // error
            )
                .prop_map(
                    |(message, title, success, package, version, registry, error)| WebhookPayload {
                        message,
                        title,
                        success,
                        package,
                        version,
                        registry,
                        error,
                        extra: std::collections::BTreeMap::new(),
                    },
                )
        }

        proptest! {
            // Payload serialization round-trip with arbitrary names/versions
            #[test]
            fn payload_roundtrip(payload in arb_payload()) {
                let json = serde_json::to_string(&payload).unwrap();
                let rt: WebhookPayload = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&rt.message, &payload.message);
                prop_assert_eq!(rt.success, payload.success);
                prop_assert_eq!(&rt.package, &payload.package);
                prop_assert_eq!(&rt.version, &payload.version);
                prop_assert_eq!(&rt.registry, &payload.registry);
                prop_assert_eq!(&rt.error, &payload.error);
                prop_assert_eq!(&rt.title, &payload.title);
            }

            // Generic JSON always contains required keys
            #[test]
            fn generic_json_has_required_keys(payload in arb_payload()) {
                let json = serde_json::to_string(&payload).unwrap();
                let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
                let obj = parsed.as_object().unwrap();
                prop_assert!(obj.contains_key("message"));
                prop_assert!(obj.contains_key("success"));
            }

            // Slack payload is always valid JSON with "attachments" array
            #[test]
            fn slack_payload_always_valid(payload in arb_payload()) {
                let json = slack_payload(&payload).unwrap();
                let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
                prop_assert!(parsed["attachments"].is_array());
                let att = &parsed["attachments"][0];
                // Color is always "good" or "danger"
                let color = att["color"].as_str().unwrap();
                prop_assert!(color == "good" || color == "danger");
            }

            // Discord payload is always valid JSON with "embeds" array
            #[test]
            fn discord_payload_always_valid(payload in arb_payload()) {
                let json = discord_payload(&payload).unwrap();
                let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
                prop_assert!(parsed["embeds"].is_array());
                let embed = &parsed["embeds"][0];
                let color = embed["color"].as_u64().unwrap();
                prop_assert!(color == 65280 || color == 16711680);
            }

            // publish_success_payload preserves arbitrary names and versions
            #[test]
            fn success_payload_preserves_inputs(
                name in package_name(),
                ver in version_string(),
                reg in "[a-z-]{1,20}",
            ) {
                let p = publish_success_payload(&name, &ver, &reg);
                prop_assert_eq!(p.package.as_deref(), Some(name.as_str()));
                prop_assert_eq!(p.version.as_deref(), Some(ver.as_str()));
                prop_assert_eq!(p.registry.as_deref(), Some(reg.as_str()));
                prop_assert!(p.success);
                prop_assert!(p.message.contains(&name));
                prop_assert!(p.message.contains(&ver));
            }

            // publish_failure_payload preserves arbitrary names and versions
            #[test]
            fn failure_payload_preserves_inputs(
                name in package_name(),
                ver in version_string(),
                err in ".*",
            ) {
                let p = publish_failure_payload(&name, &ver, &err);
                prop_assert_eq!(p.package.as_deref(), Some(name.as_str()));
                prop_assert_eq!(p.version.as_deref(), Some(ver.as_str()));
                prop_assert_eq!(p.error.as_deref(), Some(err.as_str()));
                prop_assert!(!p.success);
            }

            // HMAC signature is deterministic and well-formed
            #[test]
            fn signature_deterministic_and_wellformed(
                secret in ".{1,64}",
                body in ".*",
            ) {
                let s1 = webhook_signature(&secret, &body).unwrap();
                let s2 = webhook_signature(&secret, &body).unwrap();
                prop_assert_eq!(&s1, &s2);
                prop_assert!(s1.starts_with("sha256="));
                // sha256 hex digest is 64 chars
                prop_assert_eq!(s1.len(), "sha256=".len() + 64);
            }

            // WebhookConfig URL: various patterns never panic during serialization
            #[test]
            fn config_with_arbitrary_url_serializes(
                url in "https?://[a-z0-9.-]{1,30}(:[0-9]{1,5})?(/[a-z0-9/_-]*)?",
            ) {
                let config = WebhookConfig {
                    url: url.clone(),
                    ..Default::default()
                };
                let json = serde_json::to_string(&config).unwrap();
                let rt: WebhookConfig = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&rt.url, &url);
            }

            // Optional fields are omitted from JSON when None
            #[test]
            fn none_fields_omitted(msg in ".*", success in any::<bool>()) {
                let payload = WebhookPayload {
                    message: msg,
                    success,
                    ..Default::default()
                };
                let json = serde_json::to_string(&payload).unwrap();
                prop_assert!(!json.contains("\"title\""));
                prop_assert!(!json.contains("\"package\""));
                prop_assert!(!json.contains("\"version\""));
                prop_assert!(!json.contains("\"registry\""));
                prop_assert!(!json.contains("\"error\""));
            }

            // hex_encode produces correct length and only hex chars
            #[test]
            fn hex_encode_valid(bytes in proptest::collection::vec(any::<u8>(), 0..128)) {
                let encoded = hex_encode(&bytes);
                prop_assert_eq!(encoded.len(), bytes.len() * 2);
                prop_assert!(encoded.chars().all(|c| c.is_ascii_hexdigit()));
            }

            // Webhook URL validation: arbitrary strings never panic in send_webhook
            #[test]
            fn url_validation_never_panics(url in ".*") {
                let config = WebhookConfig {
                    url,
                    timeout_secs: 1,
                    ..Default::default()
                };
                let payload = WebhookPayload {
                    message: "test".to_string(),
                    ..Default::default()
                };
                // Must not panic — error is fine
                let _ = send_webhook(&config, &payload);
            }
        }
    }

    // --- Edge-case tests ---

    mod edge_cases {
        use super::*;

        // -- Malformed webhook URLs --

        #[test]
        fn send_webhook_empty_url_returns_error() {
            let config = WebhookConfig {
                url: String::new(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            assert!(send_webhook(&config, &payload).is_err());
        }

        #[test]
        fn send_webhook_ftp_scheme_returns_error() {
            let config = WebhookConfig {
                url: "ftp://example.com/webhook".to_string(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            assert!(send_webhook(&config, &payload).is_err());
        }

        #[test]
        fn send_webhook_missing_host_returns_error() {
            let config = WebhookConfig {
                url: "http://".to_string(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            assert!(send_webhook(&config, &payload).is_err());
        }

        #[test]
        fn send_webhook_just_scheme_returns_error() {
            let config = WebhookConfig {
                url: "https".to_string(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            assert!(send_webhook(&config, &payload).is_err());
        }

        #[test]
        fn send_webhook_whitespace_url_returns_error() {
            let config = WebhookConfig {
                url: "   ".to_string(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            assert!(send_webhook(&config, &payload).is_err());
        }

        #[tokio::test]
        async fn send_webhook_async_empty_url_returns_error() {
            let config = WebhookConfig {
                url: String::new(),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };
            assert!(send_webhook_async(&config, &payload).await.is_err());
        }

        // -- Timeout behavior --

        #[test]
        fn send_webhook_timeout_with_slow_server() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/slow"),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "timeout test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                // Delay longer than the client timeout
                std::thread::sleep(Duration::from_secs(3));
                let response = tiny_http::Response::from_string("too late");
                let _ = req.respond(response);
            });

            let result = send_webhook(&config, &payload);
            assert!(result.is_err());
            let err_msg = format!("{:#}", result.unwrap_err());
            assert!(
                err_msg.contains("timed out")
                    || err_msg.contains("timeout")
                    || err_msg.contains("Timeout")
                    || err_msg.contains("operation")
            );
            handle.join().unwrap();
        }

        #[tokio::test]
        async fn send_webhook_async_timeout_with_slow_server() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/slow"),
                timeout_secs: 1,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "async timeout test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                std::thread::sleep(Duration::from_secs(3));
                let _ = req.respond(tiny_http::Response::from_string("too late"));
            });

            let result = send_webhook_async(&config, &payload).await;
            assert!(result.is_err());
            handle.join().unwrap();
        }

        // -- Large payload body (>100KB) --

        #[test]
        fn send_webhook_large_payload_body() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/large"),
                timeout_secs: 10,
                ..Default::default()
            };

            // Build a payload with >100KB message
            let large_message = "x".repeat(110_000);
            let payload = WebhookPayload {
                message: large_message.clone(),
                success: true,
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let mut req = server.recv().unwrap();
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                assert!(body.len() > 100_000);
                let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert_eq!(parsed["message"].as_str().unwrap().len(), 110_000);
                req.respond(tiny_http::Response::from_string("ok")).unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_ok());
        }

        #[test]
        fn large_payload_serializes_correctly() {
            let large_message = "a".repeat(150_000);
            let payload = WebhookPayload {
                message: large_message.clone(),
                success: true,
                ..Default::default()
            };
            let json = serde_json::to_string(&payload).unwrap();
            assert!(json.len() > 100_000);
            let rt: WebhookPayload = serde_json::from_str(&json).unwrap();
            assert_eq!(rt.message.len(), 150_000);
        }

        // -- Unicode in webhook event data --

        #[test]
        fn payload_with_unicode_message() {
            let payload = WebhookPayload {
                message: "パッケージ公開成功 🎉".to_string(),
                title: Some("リリース通知".to_string()),
                success: true,
                package: Some("日本語パッケージ".to_string()),
                version: Some("1.0.0".to_string()),
                ..Default::default()
            };
            let json = serde_json::to_string(&payload).unwrap();
            let rt: WebhookPayload = serde_json::from_str(&json).unwrap();
            assert_eq!(rt.message, "パッケージ公開成功 🎉");
            assert_eq!(rt.title.as_deref(), Some("リリース通知"));
            assert_eq!(rt.package.as_deref(), Some("日本語パッケージ"));
        }

        #[test]
        fn slack_payload_with_unicode() {
            let payload = WebhookPayload {
                message: "Émojis: 🚀🦀✅".to_string(),
                title: Some("Ünïcödé Tïtlé".to_string()),
                success: true,
                package: Some("crâte-ñame".to_string()),
                ..Default::default()
            };
            let json = slack_payload(&payload).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed["attachments"][0]["text"], "Émojis: 🚀🦀✅");
            assert_eq!(parsed["attachments"][0]["title"], "Ünïcödé Tïtlé");
        }

        #[test]
        fn discord_payload_with_unicode() {
            let payload = WebhookPayload {
                message: "已发布 📦".to_string(),
                title: Some("发布通知".to_string()),
                success: false,
                error: Some("сетевая ошибка".to_string()),
                ..Default::default()
            };
            let json = discord_payload(&payload).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed["embeds"][0]["description"], "已发布 📦");
            assert_eq!(parsed["embeds"][0]["title"], "发布通知");
        }

        #[test]
        fn unicode_in_webhook_to_server() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/unicode"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "🦀 Rust crate published! 日本語テスト".to_string(),
                success: true,
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let mut req = server.recv().unwrap();
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
                let msg = parsed["message"].as_str().unwrap();
                assert!(msg.contains("🦀"));
                assert!(msg.contains("日本語テスト"));
                req.respond(tiny_http::Response::from_string("ok")).unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_ok());
        }

        // -- Concurrent webhook sends --

        #[test]
        fn concurrent_webhook_sends() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();
            let server = std::sync::Arc::new(server);

            let num_requests = 5;

            // Spawn server handler threads
            let mut server_handles = vec![];
            for _ in 0..num_requests {
                let srv = server.clone();
                server_handles.push(std::thread::spawn(move || {
                    let req = srv.recv().unwrap();
                    req.respond(tiny_http::Response::from_string("ok")).unwrap();
                }));
            }

            // Spawn concurrent client sends
            let mut client_handles = vec![];
            for i in 0..num_requests {
                let url = format!("http://{addr}/concurrent");
                client_handles.push(std::thread::spawn(move || {
                    let config = WebhookConfig {
                        url,
                        timeout_secs: 10,
                        ..Default::default()
                    };
                    let payload = WebhookPayload {
                        message: format!("concurrent message {i}"),
                        success: true,
                        ..Default::default()
                    };
                    send_webhook(&config, &payload)
                }));
            }

            for h in client_handles {
                let result = h.join().unwrap();
                assert!(result.is_ok());
            }
            for h in server_handles {
                h.join().unwrap();
            }
        }

        // -- Retry behavior on connection failure --

        #[test]
        fn send_webhook_to_closed_port_fails_fast() {
            // Use an ephemeral server, get its port, then drop it
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();
            drop(server); // port is now closed

            let config = WebhookConfig {
                url: format!("http://{addr}/gone"),
                timeout_secs: 2,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "should fail".to_string(),
                ..Default::default()
            };

            let start = std::time::Instant::now();
            let result = send_webhook(&config, &payload);
            let elapsed = start.elapsed();

            assert!(result.is_err());
            // Should fail within a reasonable time, not hang
            assert!(elapsed < Duration::from_secs(10));
        }

        #[test]
        fn multiple_sends_to_unreachable_all_fail() {
            let results: Vec<_> = (0..3)
                .map(|_| {
                    let config = WebhookConfig {
                        url: "http://127.0.0.1:1/unreachable".to_string(),
                        timeout_secs: 1,
                        ..Default::default()
                    };
                    let payload = WebhookPayload {
                        message: "fail".to_string(),
                        ..Default::default()
                    };
                    send_webhook(&config, &payload)
                })
                .collect();

            assert!(results.iter().all(|r| r.is_err()));
        }

        // -- HTTP status code handling --

        #[test]
        fn http_status_200_is_success() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/ok"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("ok")
                        .with_status_code(tiny_http::StatusCode(200)),
                )
                .unwrap();
            });

            assert!(send_webhook(&config, &payload).is_ok());
            handle.join().unwrap();
        }

        #[test]
        fn http_status_201_is_success() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/created"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("created")
                        .with_status_code(tiny_http::StatusCode(201)),
                )
                .unwrap();
            });

            assert!(send_webhook(&config, &payload).is_ok());
            handle.join().unwrap();
        }

        #[test]
        fn http_status_204_is_success() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/nocontent"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("")
                        .with_status_code(tiny_http::StatusCode(204)),
                )
                .unwrap();
            });

            assert!(send_webhook(&config, &payload).is_ok());
            handle.join().unwrap();
        }

        #[test]
        fn http_status_400_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/bad"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("bad request")
                        .with_status_code(tiny_http::StatusCode(400)),
                )
                .unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("400"));
        }

        #[test]
        fn http_status_401_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/unauth"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("unauthorized")
                        .with_status_code(tiny_http::StatusCode(401)),
                )
                .unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("401"));
        }

        #[test]
        fn http_status_404_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/notfound"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("not found")
                        .with_status_code(tiny_http::StatusCode(404)),
                )
                .unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("404"));
        }

        #[test]
        fn http_status_429_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/ratelimit"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("rate limited")
                        .with_status_code(tiny_http::StatusCode(429)),
                )
                .unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("429"));
        }

        #[test]
        fn http_status_502_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/badgw"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("bad gateway")
                        .with_status_code(tiny_http::StatusCode(502)),
                )
                .unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("502"));
        }

        #[test]
        fn http_status_503_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/unavail"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("service unavailable")
                        .with_status_code(tiny_http::StatusCode(503)),
                )
                .unwrap();
            });

            let result = send_webhook(&config, &payload);
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("503"));
        }

        #[tokio::test]
        async fn async_http_status_4xx_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/bad"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("bad request")
                        .with_status_code(tiny_http::StatusCode(422)),
                )
                .unwrap();
            });

            let result = send_webhook_async(&config, &payload).await;
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("422"));
        }

        #[tokio::test]
        async fn async_http_status_5xx_is_error() {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();

            let config = WebhookConfig {
                url: format!("http://{addr}/err"),
                timeout_secs: 5,
                ..Default::default()
            };
            let payload = WebhookPayload {
                message: "test".to_string(),
                ..Default::default()
            };

            let handle = std::thread::spawn(move || {
                let req = server.recv().unwrap();
                req.respond(
                    tiny_http::Response::from_string("error")
                        .with_status_code(tiny_http::StatusCode(500)),
                )
                .unwrap();
            });

            let result = send_webhook_async(&config, &payload).await;
            handle.join().unwrap();
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("500"));
        }
    }

    // --- Snapshot tests for WebhookConfig variants and event payloads ---

    mod snapshot_edge_cases {
        use super::*;

        #[test]
        fn config_generic_with_secret_snapshot() {
            let config = WebhookConfig {
                url: "https://example.com/hook".to_string(),
                webhook_type: WebhookType::Generic,
                secret: Some("top-secret".to_string()),
                timeout_secs: 15,
            };
            insta::assert_debug_snapshot!("config_generic_with_secret", config);
        }

        #[test]
        fn config_slack_no_secret_snapshot() {
            let config = WebhookConfig {
                url: "https://hooks.slack.com/services/T/B/x".to_string(),
                webhook_type: WebhookType::Slack,
                secret: None,
                timeout_secs: 30,
            };
            insta::assert_debug_snapshot!("config_slack_no_secret", config);
        }

        #[test]
        fn config_discord_with_secret_snapshot() {
            let config = WebhookConfig {
                url: "https://discord.com/api/webhooks/123/tok".to_string(),
                webhook_type: WebhookType::Discord,
                secret: Some("discord-secret".to_string()),
                timeout_secs: 45,
            };
            insta::assert_debug_snapshot!("config_discord_with_secret", config);
        }

        #[test]
        fn config_default_snapshot() {
            let config = WebhookConfig::default();
            insta::assert_debug_snapshot!("config_default_all_fields", config);
        }

        #[test]
        fn config_minimal_timeout_snapshot() {
            let config = WebhookConfig {
                url: "http://localhost:8080/webhook".to_string(),
                webhook_type: WebhookType::Generic,
                secret: None,
                timeout_secs: 1,
            };
            insta::assert_debug_snapshot!("config_minimal_timeout", config);
        }

        #[test]
        fn payload_unicode_snapshot() {
            let payload = WebhookPayload {
                message: "パッケージ公開 🚀".to_string(),
                title: Some("リリース".to_string()),
                success: true,
                package: Some("日本語crate".to_string()),
                version: Some("1.0.0".to_string()),
                ..Default::default()
            };
            insta::assert_debug_snapshot!("payload_unicode", payload);
        }

        #[test]
        fn payload_error_with_details_snapshot() {
            let payload = publish_failure_payload(
                "my-crate",
                "2.0.0",
                "connection refused: server at registry.example.com:443 not reachable",
            );
            insta::assert_debug_snapshot!("payload_error_with_details", payload);
        }

        #[test]
        fn payload_with_extra_fields_snapshot() {
            let mut extra = std::collections::BTreeMap::new();
            extra.insert("ci_provider".to_string(), serde_json::json!("github"));
            extra.insert("run_number".to_string(), serde_json::json!(42));
            extra.insert("branch".to_string(), serde_json::json!("main"));

            let payload = WebhookPayload {
                message: "Published with extras".to_string(),
                title: Some("CI Publish".to_string()),
                success: true,
                package: Some("my-crate".to_string()),
                version: Some("3.0.0".to_string()),
                registry: Some("crates-io".to_string()),
                error: None,
                extra,
            };
            insta::assert_debug_snapshot!("payload_with_extra_ci_fields", payload);
        }

        #[test]
        fn slack_unicode_payload_snapshot() {
            let payload = WebhookPayload {
                message: "🎉 Published crâte-ñame".to_string(),
                success: true,
                package: Some("crâte-ñame".to_string()),
                version: Some("1.0.0".to_string()),
                ..Default::default()
            };
            let body = slack_payload(&payload).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            insta::assert_debug_snapshot!("slack_unicode_payload", json);
        }

        #[test]
        fn discord_unicode_payload_snapshot() {
            let payload = WebhookPayload {
                message: "🦀 Опубликовано".to_string(),
                success: false,
                error: Some("сетевая ошибка".to_string()),
                ..Default::default()
            };
            let body = discord_payload(&payload).unwrap();
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            insta::assert_debug_snapshot!("discord_unicode_payload", json);
        }
    }
}
