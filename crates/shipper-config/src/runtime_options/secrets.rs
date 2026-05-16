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

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_cli() -> CliOverrides {
        CliOverrides::default()
    }

    fn empty_webhook() -> WebhookConfig {
        WebhookConfig::default()
    }

    fn empty_encryption() -> EncryptionConfigInner {
        EncryptionConfigInner::default()
    }

    #[test]
    fn resolve_webhook_returns_config_clone_when_cli_overrides_absent() {
        let mut config = empty_webhook();
        config.url = "https://hooks.example.com/abc".to_string();
        config.secret = Some("file-secret".to_string());

        let resolved = resolve_webhook(&config, &empty_cli());

        assert_eq!(resolved.url, "https://hooks.example.com/abc");
        assert_eq!(resolved.secret.as_deref(), Some("file-secret"));
    }

    #[test]
    fn resolve_webhook_cli_url_overrides_config_url() {
        let mut config = empty_webhook();
        config.url = "https://hooks.example.com/from-config".to_string();

        let mut cli = empty_cli();
        cli.webhook_url = Some("https://hooks.example.com/from-cli".to_string());

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.url, "https://hooks.example.com/from-cli");
    }

    #[test]
    fn resolve_webhook_cli_secret_overrides_config_secret() {
        let mut config = empty_webhook();
        config.secret = Some("file-secret".to_string());

        let mut cli = empty_cli();
        cli.webhook_secret = Some("cli-secret".to_string());

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.secret.as_deref(), Some("cli-secret"));
    }

    #[test]
    fn resolve_webhook_cli_secret_can_introduce_secret_when_config_has_none() {
        let config = empty_webhook();
        assert!(config.secret.is_none());

        let mut cli = empty_cli();
        cli.webhook_secret = Some("only-from-cli".to_string());

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.secret.as_deref(), Some("only-from-cli"));
    }

    #[test]
    fn resolve_webhook_preserves_non_overridden_fields() {
        let mut config = empty_webhook();
        config.url = "https://hooks.example.com/keep".to_string();
        config.secret = Some("keep-me".to_string());
        config.timeout_secs = 99;

        let resolved = resolve_webhook(&config, &empty_cli());

        assert_eq!(resolved.url, "https://hooks.example.com/keep");
        assert_eq!(resolved.secret.as_deref(), Some("keep-me"));
        assert_eq!(resolved.timeout_secs, 99);
    }

    #[test]
    fn resolve_encryption_disabled_by_default_with_no_overrides() {
        let resolved = resolve_encryption(&empty_encryption(), &empty_cli());

        assert!(!resolved.enabled);
        assert!(resolved.passphrase.is_none());
        assert!(resolved.env_var.is_none());
    }

    #[test]
    fn resolve_encryption_cli_encrypt_flag_enables_when_config_disabled() {
        let mut cli = empty_cli();
        cli.encrypt = true;

        let resolved = resolve_encryption(&empty_encryption(), &cli);

        assert!(resolved.enabled);
    }

    #[test]
    fn resolve_encryption_config_enabled_alone_enables_without_cli_flag() {
        let mut config = empty_encryption();
        config.enabled = true;

        let resolved = resolve_encryption(&config, &empty_cli());

        assert!(resolved.enabled);
    }

    #[test]
    fn resolve_encryption_cli_passphrase_wins_over_config_passphrase() {
        let mut config = empty_encryption();
        config.enabled = true;
        config.passphrase = Some("from-config".to_string());

        let mut cli = empty_cli();
        cli.encrypt_passphrase = Some("from-cli".to_string());

        let resolved = resolve_encryption(&config, &cli);

        assert_eq!(resolved.passphrase.as_deref(), Some("from-cli"));
    }

    #[test]
    fn resolve_encryption_falls_back_to_config_passphrase_when_cli_unset() {
        let mut config = empty_encryption();
        config.enabled = true;
        config.passphrase = Some("from-config".to_string());

        let resolved = resolve_encryption(&config, &empty_cli());

        assert_eq!(resolved.passphrase.as_deref(), Some("from-config"));
    }

    #[test]
    fn resolve_encryption_defaults_env_var_when_enabled_without_passphrase() {
        let mut cli = empty_cli();
        cli.encrypt = true;

        let resolved = resolve_encryption(&empty_encryption(), &cli);

        assert!(resolved.enabled);
        assert!(resolved.passphrase.is_none());
        assert_eq!(resolved.env_var.as_deref(), Some("SHIPPER_ENCRYPT_KEY"));
    }

    #[test]
    fn resolve_encryption_does_not_default_env_var_when_passphrase_present() {
        let mut cli = empty_cli();
        cli.encrypt = true;
        cli.encrypt_passphrase = Some("inline-passphrase".to_string());

        let resolved = resolve_encryption(&empty_encryption(), &cli);

        assert!(resolved.enabled);
        assert!(resolved.passphrase.is_some());
        assert!(resolved.env_var.is_none());
    }

    #[test]
    fn resolve_encryption_does_not_default_env_var_when_disabled() {
        let resolved = resolve_encryption(&empty_encryption(), &empty_cli());

        assert!(!resolved.enabled);
        assert!(resolved.env_var.is_none());
    }

    #[test]
    fn resolve_encryption_explicit_env_key_overrides_default() {
        let mut config = empty_encryption();
        config.enabled = true;
        config.env_key = Some("CUSTOM_ENCRYPT_VAR".to_string());

        let resolved = resolve_encryption(&config, &empty_cli());

        assert_eq!(resolved.env_var.as_deref(), Some("CUSTOM_ENCRYPT_VAR"));
    }

    #[test]
    fn resolve_encryption_explicit_env_key_is_kept_even_with_passphrase() {
        let mut config = empty_encryption();
        config.enabled = true;
        config.passphrase = Some("p".to_string());
        config.env_key = Some("CUSTOM_KEY".to_string());

        let resolved = resolve_encryption(&config, &empty_cli());

        assert_eq!(resolved.passphrase.as_deref(), Some("p"));
        assert_eq!(resolved.env_var.as_deref(), Some("CUSTOM_KEY"));
    }

    #[test]
    fn resolve_encryption_both_sources_enable_uses_or_semantics() {
        let mut config = empty_encryption();
        config.enabled = true;

        let mut cli = empty_cli();
        cli.encrypt = true;

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled);
    }
}
