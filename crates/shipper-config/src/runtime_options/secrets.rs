use shipper_encrypt::EncryptionConfig as EncryptionSettings;
use shipper_webhook::WebhookConfig;

use crate::{CliOverrides, EncryptionConfigInner};

pub(super) fn resolve_webhook(config: &WebhookConfig, cli: &CliOverrides) -> WebhookConfig {
    let mut resolved = config.clone();

    if let Some(url) = &cli.webhook_url {
        resolved.url = url.clone();
    }
    if let Some(secret) = &cli.webhook_secret {
        resolved.secret = Some(secret.clone());
    }

    resolved
}

pub(super) fn resolve_encryption(
    config: &EncryptionConfigInner,
    cli: &CliOverrides,
) -> EncryptionSettings {
    let mut resolved = EncryptionSettings::default();

    resolved.enabled = cli.encrypt || config.enabled;
    resolved.passphrase = cli
        .encrypt_passphrase
        .clone()
        .or_else(|| config.passphrase.clone());
    resolved.env_var = config
        .env_key
        .clone()
        .or_else(|| default_env_var(&resolved));

    resolved
}

fn default_env_var(config: &EncryptionSettings) -> Option<String> {
    if config.enabled && config.passphrase.is_none() {
        Some("SHIPPER_ENCRYPT_KEY".to_string())
    } else {
        None
    }
}
