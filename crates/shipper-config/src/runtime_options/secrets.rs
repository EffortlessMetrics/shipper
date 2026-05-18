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
    use shipper_webhook::WebhookType;

    use crate::CliOverrides;

    fn empty_cli() -> CliOverrides {
        CliOverrides::default()
    }

    fn cfg_webhook(url: &str) -> WebhookConfig {
        WebhookConfig {
            url: url.to_string(),
            webhook_type: WebhookType::Generic,
            secret: Some("config-secret".to_string()),
            timeout_secs: 30,
        }
    }

    // ── resolve_webhook ────────────────────────────────────────────────────

    #[test]
    fn resolve_webhook_returns_config_when_no_overrides() {
        let config = cfg_webhook("https://config.example/hook");
        let cli = empty_cli();

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.url, "https://config.example/hook");
        assert_eq!(resolved.secret.as_deref(), Some("config-secret"));
    }

    #[test]
    fn resolve_webhook_url_override_replaces_config_url() {
        let config = cfg_webhook("https://config.example/hook");
        let cli = CliOverrides {
            webhook_url: Some("https://cli.example/hook".to_string()),
            ..empty_cli()
        };

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.url, "https://cli.example/hook");
        assert_eq!(
            resolved.secret.as_deref(),
            Some("config-secret"),
            "url override must not touch secret"
        );
    }

    #[test]
    fn resolve_webhook_secret_override_replaces_config_secret() {
        let config = cfg_webhook("https://config.example/hook");
        let cli = CliOverrides {
            webhook_secret: Some("cli-secret".to_string()),
            ..empty_cli()
        };

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.url, "https://config.example/hook");
        assert_eq!(resolved.secret.as_deref(), Some("cli-secret"));
    }

    #[test]
    fn resolve_webhook_both_overrides_apply_together() {
        let config = cfg_webhook("https://config.example/hook");
        let cli = CliOverrides {
            webhook_url: Some("https://cli.example/hook".to_string()),
            webhook_secret: Some("cli-secret".to_string()),
            ..empty_cli()
        };

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.url, "https://cli.example/hook");
        assert_eq!(resolved.secret.as_deref(), Some("cli-secret"));
    }

    #[test]
    fn resolve_webhook_preserves_webhook_type_and_timeout() {
        let config = WebhookConfig {
            url: "https://config.example/hook".to_string(),
            webhook_type: WebhookType::Slack,
            secret: None,
            timeout_secs: 7,
        };
        let cli = CliOverrides {
            webhook_url: Some("https://cli.example/hook".to_string()),
            ..empty_cli()
        };

        let resolved = resolve_webhook(&config, &cli);

        assert_eq!(resolved.webhook_type, WebhookType::Slack);
        assert_eq!(resolved.timeout_secs, 7);
    }

    // ── resolve_encryption ─────────────────────────────────────────────────

    #[test]
    fn resolve_encryption_disabled_when_nothing_set() {
        let config = EncryptionConfigInner::default();
        let cli = empty_cli();

        let resolved = resolve_encryption(&config, &cli);

        assert!(!resolved.enabled);
        assert!(resolved.passphrase.is_none());
        assert!(
            resolved.env_var.is_none(),
            "env_var must not default when encryption is disabled"
        );
    }

    #[test]
    fn resolve_encryption_cli_flag_enables_and_implies_default_env_var() {
        let config = EncryptionConfigInner::default();
        let cli = CliOverrides {
            encrypt: true,
            ..empty_cli()
        };

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled);
        assert!(resolved.passphrase.is_none());
        assert_eq!(resolved.env_var.as_deref(), Some("SHIPPER_ENCRYPT_KEY"));
    }

    #[test]
    fn resolve_encryption_config_enable_alone_implies_default_env_var() {
        let config = EncryptionConfigInner {
            enabled: true,
            passphrase: None,
            env_key: None,
        };
        let cli = empty_cli();

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled);
        assert!(resolved.passphrase.is_none());
        assert_eq!(resolved.env_var.as_deref(), Some("SHIPPER_ENCRYPT_KEY"));
    }

    #[test]
    fn resolve_encryption_cli_passphrase_overrides_config_passphrase() {
        let config = EncryptionConfigInner {
            enabled: true,
            passphrase: Some("config-pass".to_string()),
            env_key: None,
        };
        let cli = CliOverrides {
            encrypt: true,
            encrypt_passphrase: Some("cli-pass".to_string()),
            ..empty_cli()
        };

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled);
        assert_eq!(resolved.passphrase.as_deref(), Some("cli-pass"));
        assert!(
            resolved.env_var.is_none(),
            "explicit passphrase suppresses default env var"
        );
    }

    #[test]
    fn resolve_encryption_falls_back_to_config_passphrase_when_cli_absent() {
        let config = EncryptionConfigInner {
            enabled: true,
            passphrase: Some("config-pass".to_string()),
            env_key: None,
        };
        let cli = empty_cli();

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled);
        assert_eq!(resolved.passphrase.as_deref(), Some("config-pass"));
        assert!(resolved.env_var.is_none());
    }

    #[test]
    fn resolve_encryption_config_env_key_preserved_over_default() {
        let config = EncryptionConfigInner {
            enabled: true,
            passphrase: None,
            env_key: Some("MY_CUSTOM_KEY".to_string()),
        };
        let cli = empty_cli();

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled);
        assert_eq!(resolved.env_var.as_deref(), Some("MY_CUSTOM_KEY"));
    }

    #[test]
    fn resolve_encryption_config_env_key_is_preserved_with_passphrase() {
        let config = EncryptionConfigInner {
            enabled: true,
            passphrase: Some("config-pass".to_string()),
            env_key: Some("MY_CUSTOM_KEY".to_string()),
        };
        let cli = empty_cli();

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled);
        assert_eq!(resolved.passphrase.as_deref(), Some("config-pass"));
        assert_eq!(resolved.env_var.as_deref(), Some("MY_CUSTOM_KEY"));
    }

    #[test]
    fn resolve_encryption_cli_flag_is_or_with_config_enable() {
        let config = EncryptionConfigInner {
            enabled: true,
            passphrase: None,
            env_key: None,
        };
        let cli = CliOverrides {
            encrypt: false,
            ..empty_cli()
        };

        let resolved = resolve_encryption(&config, &cli);

        assert!(resolved.enabled, "config-only enable must turn it on");
    }

    // ── default_env_var (private) ──────────────────────────────────────────

    #[test]
    fn default_env_var_none_when_disabled() {
        let cfg = EncryptionSettings {
            enabled: false,
            passphrase: None,
            env_var: None,
        };
        assert!(default_env_var(&cfg).is_none());
    }

    #[test]
    fn default_env_var_none_when_passphrase_set() {
        let cfg = EncryptionSettings {
            enabled: true,
            passphrase: Some("p".to_string()),
            env_var: None,
        };
        assert!(default_env_var(&cfg).is_none());
    }

    #[test]
    fn default_env_var_default_key_when_enabled_without_passphrase() {
        let cfg = EncryptionSettings {
            enabled: true,
            passphrase: None,
            env_var: None,
        };
        assert_eq!(
            default_env_var(&cfg).as_deref(),
            Some("SHIPPER_ENCRYPT_KEY")
        );
    }
}
