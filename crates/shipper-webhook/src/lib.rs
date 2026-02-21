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
use serde::{Deserialize, Serialize};
use serde_json::json;

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
    /// Optional secret for signing (not yet implemented)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 { 30 }

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

    let response = client
        .post(&config.url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .context("failed to send webhook request")?;

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

    let response = client
        .post(&config.url)
        .header("Content-Type", "application/json")
        .body(body)
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
    let color = if payload.success { 65280_u32 } else { 16711680_u32 };
    
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
}