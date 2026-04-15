pub use shipper_config::runtime::*;
pub use shipper_config::*;

#[cfg(test)]
mod tests {
    use super::*;
    use shipper_retry::{PerErrorConfig, RetryPolicy, RetryStrategyType};
    use std::path::PathBuf;
    use std::time::Duration;

    // ── TOML parsing edge cases ─────────────────────────────────────

    #[test]
    fn comments_only_toml_parses_to_defaults() {
        let toml_str = r#"
# This file has only comments
# [policy]
# mode = "fast"
# No actual keys or sections
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.policy.mode, PublishPolicy::Safe);
        assert_eq!(config.verify.mode, VerifyMode::Workspace);
        assert_eq!(config.output.lines, 50);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn empty_sections_parse_to_section_defaults() {
        let toml_str = r#"
[policy]

[verify]

[readiness]

[output]

[lock]

[retry]

[flags]

[parallel]
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.policy.mode, PublishPolicy::Safe);
        assert_eq!(config.verify.mode, VerifyMode::Workspace);
        assert!(config.readiness.enabled);
        assert_eq!(config.output.lines, 50);
        assert_eq!(config.lock.timeout, Duration::from_secs(3600));
        assert_eq!(config.retry.max_attempts, 6);
        assert!(!config.flags.allow_dirty);
        assert!(!config.parallel.enabled);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn whitespace_only_toml_parses_to_defaults() {
        let config: ShipperConfig = toml::from_str("   \n\n  \t  \n").unwrap();
        assert_eq!(config.policy.mode, PublishPolicy::Safe);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn inline_comments_after_values_are_handled() {
        let toml_str = r#"
[policy]
mode = "fast" # override to fast

[output]
lines = 200 # more lines for CI
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.policy.mode, PublishPolicy::Fast);
        assert_eq!(config.output.lines, 200);
    }

    // ── Invalid typed values ────────────────────────────────────────

    #[test]
    fn string_for_integer_field_is_rejected() {
        let toml_str = r#"
[output]
lines = "not_a_number"
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn integer_for_string_field_is_rejected() {
        let toml_str = r#"
[policy]
mode = 42
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn boolean_for_string_field_is_rejected() {
        let toml_str = r#"
[policy]
mode = true
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_duration_format_is_rejected() {
        let toml_str = r#"
[lock]
timeout = "not_a_duration"
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn negative_integer_for_unsigned_field_is_rejected() {
        let toml_str = r#"
[retry]
max_attempts = -1
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn float_for_integer_field_is_rejected() {
        let toml_str = r#"
[output]
lines = 3.14
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_verify_mode_is_rejected() {
        let toml_str = r#"
[verify]
mode = "ultra"
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_readiness_method_is_rejected() {
        let toml_str = r#"
[readiness]
method = "magic"
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_retry_strategy_is_rejected() {
        let toml_str = r#"
[retry]
strategy = "fibonacci"
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_retry_policy_is_rejected() {
        let toml_str = r#"
[retry]
policy = "insane"
"#;
        let result: Result<ShipperConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    // ── Config file not found / load graceful defaults ──────────────

    #[test]
    fn load_from_workspace_no_config_returns_none() {
        let td = tempfile::tempdir().unwrap();
        let result = ShipperConfig::load_from_workspace(td.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_from_file_nonexistent_returns_error() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("nonexistent.toml");
        let result = ShipperConfig::load_from_file(&path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to read config file"),
            "unexpected error: {err_msg}"
        );
    }

    #[test]
    fn load_from_workspace_with_empty_file_returns_defaults() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join(".shipper.toml");
        std::fs::write(&path, "").unwrap();
        let config = ShipperConfig::load_from_workspace(td.path())
            .unwrap()
            .unwrap();
        assert_eq!(config.policy.mode, PublishPolicy::Safe);
        assert_eq!(config.output.lines, 50);
    }

    #[test]
    fn load_from_file_with_valid_content_succeeds() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("custom.toml");
        std::fs::write(
            &path,
            r#"
[policy]
mode = "fast"

[output]
lines = 123
"#,
        )
        .unwrap();
        let config = ShipperConfig::load_from_file(&path).unwrap();
        assert_eq!(config.policy.mode, PublishPolicy::Fast);
        assert_eq!(config.output.lines, 123);
    }

    #[test]
    fn load_from_file_with_invalid_schema_version_errors() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("bad_schema.toml");
        std::fs::write(&path, r#"schema_version = "not.a.valid.schema""#).unwrap();
        let result = ShipperConfig::load_from_file(&path);
        assert!(result.is_err());
    }

    // ── CLI flag override precedence ────────────────────────────────

    #[test]
    fn cli_overrides_all_option_fields() {
        let config = ShipperConfig {
            policy: PolicyConfig {
                mode: PublishPolicy::Safe,
            },
            verify: VerifyConfig {
                mode: VerifyMode::Workspace,
            },
            retry: RetryConfig {
                policy: RetryPolicy::Custom,
                max_attempts: 3,
                base_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(60),
                strategy: RetryStrategyType::Linear,
                jitter: 0.2,
                per_error: PerErrorConfig::default(),
            },
            output: OutputConfig { lines: 25 },
            lock: LockConfig {
                timeout: Duration::from_secs(600),
            },
            readiness: ReadinessConfig {
                method: ReadinessMethod::Api,
                ..ReadinessConfig::default()
            },
            state_dir: Some(PathBuf::from("config-state")),
            ..ShipperConfig::default()
        };

        let cli = CliOverrides {
            policy: Some(PublishPolicy::Fast),
            verify_mode: Some(VerifyMode::None),
            max_attempts: Some(99),
            base_delay: Some(Duration::from_millis(100)),
            max_delay: Some(Duration::from_secs(10)),
            retry_strategy: Some(RetryStrategyType::Constant),
            retry_jitter: Some(0.9),
            output_lines: Some(500),
            lock_timeout: Some(Duration::from_secs(7200)),
            state_dir: Some(PathBuf::from("cli-state")),
            readiness_method: Some(ReadinessMethod::Both),
            readiness_timeout: Some(Duration::from_secs(999)),
            readiness_poll: Some(Duration::from_secs(15)),
            ..Default::default()
        };

        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.policy, PublishPolicy::Fast);
        assert_eq!(opts.verify_mode, VerifyMode::None);
        assert_eq!(opts.max_attempts, 99);
        assert_eq!(opts.base_delay, Duration::from_millis(100));
        assert_eq!(opts.max_delay, Duration::from_secs(10));
        assert_eq!(opts.retry_strategy, RetryStrategyType::Constant);
        assert!((opts.retry_jitter - 0.9).abs() < f64::EPSILON);
        assert_eq!(opts.output_lines, 500);
        assert_eq!(opts.lock_timeout, Duration::from_secs(7200));
        assert_eq!(opts.state_dir, PathBuf::from("cli-state"));
        assert_eq!(opts.readiness.method, ReadinessMethod::Both);
        assert_eq!(opts.readiness.max_total_wait, Duration::from_secs(999));
        assert_eq!(opts.readiness.poll_interval, Duration::from_secs(15));
    }

    #[test]
    fn state_dir_precedence_cli_over_config_over_default() {
        // Default: no state_dir in config, no CLI → ".shipper"
        let config_none = ShipperConfig::default();
        let opts = config_none.build_runtime_options(CliOverrides::default());
        assert_eq!(opts.state_dir, PathBuf::from(".shipper"));

        // Config provides state_dir, CLI doesn't → config value
        let config_some = ShipperConfig {
            state_dir: Some(PathBuf::from("my-state")),
            ..ShipperConfig::default()
        };
        let opts = config_some.build_runtime_options(CliOverrides::default());
        assert_eq!(opts.state_dir, PathBuf::from("my-state"));

        // CLI overrides config state_dir
        let cli = CliOverrides {
            state_dir: Some(PathBuf::from("cli-dir")),
            ..Default::default()
        };
        let opts = config_some.build_runtime_options(cli);
        assert_eq!(opts.state_dir, PathBuf::from("cli-dir"));
    }

    // ── Retry policy preset vs custom ───────────────────────────────

    #[test]
    fn non_custom_retry_policy_ignores_config_retry_values() {
        let config = ShipperConfig {
            retry: RetryConfig {
                policy: RetryPolicy::Default,
                // These should be ignored because policy != Custom
                max_attempts: 999,
                base_delay: Duration::from_secs(999),
                max_delay: Duration::from_secs(9999),
                strategy: RetryStrategyType::Constant,
                jitter: 0.99,
                per_error: PerErrorConfig::default(),
            },
            ..ShipperConfig::default()
        };

        let opts = config.build_runtime_options(CliOverrides::default());
        // Should use the "default" policy effective values, not the raw config values
        let effective = RetryPolicy::Default.to_config();
        assert_eq!(opts.max_attempts, effective.max_attempts);
        assert_eq!(opts.base_delay, effective.base_delay);
        assert_eq!(opts.max_delay, effective.max_delay);
        assert_eq!(opts.retry_strategy, effective.strategy);
    }

    #[test]
    fn custom_retry_policy_uses_config_values() {
        let config = ShipperConfig {
            retry: RetryConfig {
                policy: RetryPolicy::Custom,
                max_attempts: 42,
                base_delay: Duration::from_millis(750),
                max_delay: Duration::from_secs(45),
                strategy: RetryStrategyType::Linear,
                jitter: 0.3,
                per_error: PerErrorConfig::default(),
            },
            ..ShipperConfig::default()
        };

        let opts = config.build_runtime_options(CliOverrides::default());
        assert_eq!(opts.max_attempts, 42);
        assert_eq!(opts.base_delay, Duration::from_millis(750));
        assert_eq!(opts.max_delay, Duration::from_secs(45));
        assert_eq!(opts.retry_strategy, RetryStrategyType::Linear);
        assert!((opts.retry_jitter - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn cli_overrides_win_over_non_custom_policy_effective_values() {
        let config = ShipperConfig {
            retry: RetryConfig {
                policy: RetryPolicy::Aggressive,
                ..RetryConfig::default()
            },
            ..ShipperConfig::default()
        };

        let cli = CliOverrides {
            max_attempts: Some(1),
            base_delay: Some(Duration::from_millis(50)),
            ..Default::default()
        };

        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.max_attempts, 1);
        assert_eq!(opts.base_delay, Duration::from_millis(50));
    }

    // ── Boolean OR semantics ────────────────────────────────────────

    #[test]
    fn boolean_flags_or_semantics_all_combinations() {
        for (cfg_dirty, cli_dirty) in [(false, false), (false, true), (true, false), (true, true)] {
            let config = ShipperConfig {
                flags: FlagsConfig {
                    allow_dirty: cfg_dirty,
                    ..Default::default()
                },
                ..ShipperConfig::default()
            };
            let cli = CliOverrides {
                allow_dirty: cli_dirty,
                ..Default::default()
            };
            let opts = config.build_runtime_options(cli);
            assert_eq!(
                opts.allow_dirty,
                cfg_dirty || cli_dirty,
                "cfg={cfg_dirty}, cli={cli_dirty}"
            );
        }
    }

    #[test]
    fn parallel_enabled_or_semantics() {
        let config = ShipperConfig {
            parallel: ParallelConfig {
                enabled: true,
                ..ParallelConfig::default()
            },
            ..ShipperConfig::default()
        };
        // CLI doesn't enable parallel, but config does → enabled
        let opts = config.build_runtime_options(CliOverrides::default());
        assert!(opts.parallel.enabled);

        // CLI enables parallel, config doesn't
        let config2 = ShipperConfig::default();
        let cli = CliOverrides {
            parallel_enabled: true,
            ..Default::default()
        };
        let opts2 = config2.build_runtime_options(cli);
        assert!(opts2.parallel.enabled);
    }

    #[test]
    fn encryption_enabled_or_semantics() {
        // Config enables, CLI doesn't
        let config = ShipperConfig {
            encryption: EncryptionConfigInner {
                enabled: true,
                passphrase: Some("cfg-pass".to_string()),
                env_key: None,
            },
            ..ShipperConfig::default()
        };
        let opts = config.build_runtime_options(CliOverrides::default());
        assert!(opts.encryption.enabled);
        assert_eq!(opts.encryption.passphrase.as_deref(), Some("cfg-pass"));

        // CLI enables, config doesn't
        let config2 = ShipperConfig::default();
        let cli = CliOverrides {
            encrypt: true,
            encrypt_passphrase: Some("cli-pass".to_string()),
            ..Default::default()
        };
        let opts2 = config2.build_runtime_options(cli);
        assert!(opts2.encryption.enabled);
        assert_eq!(opts2.encryption.passphrase.as_deref(), Some("cli-pass"));
    }

    // ── Readiness CLI overrides ─────────────────────────────────────

    #[test]
    fn no_readiness_cli_flag_disables_even_if_config_enables() {
        let config = ShipperConfig {
            readiness: ReadinessConfig {
                enabled: true,
                ..ReadinessConfig::default()
            },
            ..ShipperConfig::default()
        };
        let cli = CliOverrides {
            no_readiness: true,
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert!(!opts.readiness.enabled);
    }

    #[test]
    fn readiness_config_only_fields_passthrough() {
        let config = ShipperConfig {
            readiness: ReadinessConfig {
                enabled: true,
                initial_delay: Duration::from_secs(10),
                max_delay: Duration::from_secs(120),
                jitter_factor: 0.75,
                index_path: Some(PathBuf::from("/custom/index")),
                prefer_index: true,
                ..ReadinessConfig::default()
            },
            ..ShipperConfig::default()
        };
        let opts = config.build_runtime_options(CliOverrides::default());
        // These fields are config-only (no CLI override)
        assert_eq!(opts.readiness.initial_delay, Duration::from_secs(10));
        assert_eq!(opts.readiness.max_delay, Duration::from_secs(120));
        assert!((opts.readiness.jitter_factor - 0.75).abs() < f64::EPSILON);
        assert_eq!(
            opts.readiness.index_path,
            Some(PathBuf::from("/custom/index"))
        );
        assert!(opts.readiness.prefer_index);
    }

    // ── Webhook CLI overrides ───────────────────────────────────────

    #[test]
    fn webhook_cli_overrides_url_and_secret() {
        let config = ShipperConfig {
            webhook: WebhookConfig {
                url: "https://config-url.example.com".to_string(),
                secret: Some("config-secret".to_string()),
                ..WebhookConfig::default()
            },
            ..ShipperConfig::default()
        };
        let cli = CliOverrides {
            webhook_url: Some("https://cli-url.example.com".to_string()),
            webhook_secret: Some("cli-secret".to_string()),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.webhook.url, "https://cli-url.example.com");
        assert_eq!(opts.webhook.secret.as_deref(), Some("cli-secret"));
    }

    #[test]
    fn webhook_cli_partial_override_only_url() {
        let config = ShipperConfig {
            webhook: WebhookConfig {
                url: "https://config-url.example.com".to_string(),
                secret: Some("config-secret".to_string()),
                ..WebhookConfig::default()
            },
            ..ShipperConfig::default()
        };
        let cli = CliOverrides {
            webhook_url: Some("https://cli-url.example.com".to_string()),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.webhook.url, "https://cli-url.example.com");
        // Secret from config is preserved
        assert_eq!(opts.webhook.secret.as_deref(), Some("config-secret"));
    }

    // ── Registry CLI merging ────────────────────────────────────────

    #[test]
    fn all_registries_cli_flag_selects_all_configured() {
        let config = ShipperConfig {
            registries: MultiRegistryConfig {
                registries: vec![
                    RegistryConfig {
                        name: "reg-a".to_string(),
                        api_base: "https://a.example.com".to_string(),
                        index_base: None,
                        token: None,
                        default: true,
                    },
                    RegistryConfig {
                        name: "reg-b".to_string(),
                        api_base: "https://b.example.com".to_string(),
                        index_base: None,
                        token: None,
                        default: false,
                    },
                ],
                default_registries: vec![],
            },
            ..ShipperConfig::default()
        };
        let cli = CliOverrides {
            all_registries: true,
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.registries.len(), 2);
        assert_eq!(opts.registries[0].name, "reg-a");
        assert_eq!(opts.registries[1].name, "reg-b");
    }

    #[test]
    fn specific_registries_cli_flag_selects_named() {
        let config = ShipperConfig {
            registries: MultiRegistryConfig {
                registries: vec![
                    RegistryConfig {
                        name: "reg-a".to_string(),
                        api_base: "https://a.example.com".to_string(),
                        index_base: Some("https://index.a.example.com".to_string()),
                        token: None,
                        default: true,
                    },
                    RegistryConfig {
                        name: "reg-b".to_string(),
                        api_base: "https://b.example.com".to_string(),
                        index_base: None,
                        token: None,
                        default: false,
                    },
                ],
                default_registries: vec![],
            },
            ..ShipperConfig::default()
        };
        let cli = CliOverrides {
            registries: Some(vec!["reg-b".to_string()]),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.registries.len(), 1);
        assert_eq!(opts.registries[0].name, "reg-b");
        assert_eq!(opts.registries[0].api_base, "https://b.example.com");
    }

    #[test]
    fn unknown_registry_name_in_cli_gets_default_url() {
        let config = ShipperConfig::default();
        let cli = CliOverrides {
            registries: Some(vec!["crates-io".to_string()]),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.registries.len(), 1);
        assert_eq!(opts.registries[0].name, "crates-io");
        assert_eq!(opts.registries[0].api_base, "https://crates.io");
    }

    #[test]
    fn unknown_non_crates_io_registry_in_cli_gets_synthesized_url() {
        let config = ShipperConfig::default();
        let cli = CliOverrides {
            registries: Some(vec!["custom-mirror".to_string()]),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.registries.len(), 1);
        assert_eq!(opts.registries[0].name, "custom-mirror");
        // Synthesized URL pattern
        assert!(opts.registries[0].api_base.contains("custom-mirror"));
    }

    #[test]
    fn no_registry_cli_flags_yields_empty_registries() {
        let config = ShipperConfig::default();
        let cli = CliOverrides::default();
        let opts = config.build_runtime_options(cli);
        assert!(opts.registries.is_empty());
    }

    // ── Force / no_verify / resume_from CLI flags ───────────────────

    #[test]
    fn force_and_force_resume_passthrough() {
        let config = ShipperConfig::default();
        let cli = CliOverrides {
            force: true,
            force_resume: true,
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert!(opts.force);
        assert!(opts.force_resume);
    }

    #[test]
    fn no_verify_passthrough() {
        let config = ShipperConfig::default();
        let cli = CliOverrides {
            no_verify: true,
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert!(opts.no_verify);
    }

    #[test]
    fn resume_from_passthrough() {
        let config = ShipperConfig::default();
        let cli = CliOverrides {
            resume_from: Some("my-crate".to_string()),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.resume_from.as_deref(), Some("my-crate"));
    }

    #[test]
    fn resume_from_none_by_default() {
        let config = ShipperConfig::default();
        let opts = config.build_runtime_options(CliOverrides::default());
        assert!(opts.resume_from.is_none());
    }

    // ── Parallel CLI overrides ──────────────────────────────────────

    #[test]
    fn per_package_timeout_cli_override() {
        let config = ShipperConfig {
            parallel: ParallelConfig {
                enabled: true,
                max_concurrent: 4,
                per_package_timeout: Duration::from_secs(1800),
            },
            ..ShipperConfig::default()
        };
        let cli = CliOverrides {
            per_package_timeout: Some(Duration::from_secs(60)),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.parallel.per_package_timeout, Duration::from_secs(60));
    }

    // ── Encryption env_key passthrough ──────────────────────────────

    #[test]
    fn encryption_custom_env_key_from_config() {
        let config = ShipperConfig {
            encryption: EncryptionConfigInner {
                enabled: true,
                passphrase: None,
                env_key: Some("CUSTOM_KEY_VAR".to_string()),
            },
            ..ShipperConfig::default()
        };
        let opts = config.build_runtime_options(CliOverrides::default());
        assert!(opts.encryption.enabled);
        assert_eq!(opts.encryption.env_var.as_deref(), Some("CUSTOM_KEY_VAR"));
    }

    #[test]
    fn encryption_disabled_by_default() {
        let config = ShipperConfig::default();
        let opts = config.build_runtime_options(CliOverrides::default());
        assert!(!opts.encryption.enabled);
    }

    // ── TOML parsing: all sections with full content ────────────────

    #[test]
    fn parse_all_sections_simultaneously() {
        let toml_str = r#"
schema_version = "shipper.config.v1"

[policy]
mode = "balanced"

[verify]
mode = "package"

[readiness]
enabled = true
method = "both"
initial_delay = "3s"
max_delay = "90s"
max_total_wait = "10m"
poll_interval = "5s"
jitter_factor = 0.3

[output]
lines = 75

[lock]
timeout = "2h"

[retry]
policy = "custom"
max_attempts = 8
base_delay = "3s"
max_delay = "90s"
strategy = "exponential"
jitter = 0.4

[flags]
allow_dirty = true
skip_ownership_check = false
strict_ownership = true

[parallel]
enabled = true
max_concurrent = 6
per_package_timeout = "45m"

[registry]
name = "my-reg"
api_base = "https://my-reg.example.com"
index_base = "https://index.my-reg.example.com"

[encryption]
enabled = true
passphrase = "my-pass"
env_key = "MY_KEY"

[storage]
storage_type = "S3"
bucket = "releases"
region = "eu-west-1"
base_path = "artifacts/"
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.schema_version, "shipper.config.v1");
        assert_eq!(config.policy.mode, PublishPolicy::Balanced);
        assert_eq!(config.verify.mode, VerifyMode::Package);
        assert!(config.readiness.enabled);
        assert_eq!(config.readiness.method, ReadinessMethod::Both);
        assert_eq!(config.readiness.initial_delay, Duration::from_secs(3));
        assert_eq!(config.readiness.max_delay, Duration::from_secs(90));
        assert_eq!(config.readiness.max_total_wait, Duration::from_secs(600));
        assert_eq!(config.readiness.poll_interval, Duration::from_secs(5));
        assert!((config.readiness.jitter_factor - 0.3).abs() < f64::EPSILON);
        assert_eq!(config.output.lines, 75);
        assert_eq!(config.lock.timeout, Duration::from_secs(7200));
        assert_eq!(config.retry.policy, RetryPolicy::Custom);
        assert_eq!(config.retry.max_attempts, 8);
        assert_eq!(config.retry.base_delay, Duration::from_secs(3));
        assert_eq!(config.retry.max_delay, Duration::from_secs(90));
        assert_eq!(config.retry.strategy, RetryStrategyType::Exponential);
        assert!((config.retry.jitter - 0.4).abs() < f64::EPSILON);
        assert!(config.flags.allow_dirty);
        assert!(!config.flags.skip_ownership_check);
        assert!(config.flags.strict_ownership);
        assert!(config.parallel.enabled);
        assert_eq!(config.parallel.max_concurrent, 6);
        assert_eq!(
            config.parallel.per_package_timeout,
            Duration::from_secs(2700)
        );
        let reg = config.registry.as_ref().unwrap();
        assert_eq!(reg.name, "my-reg");
        assert_eq!(reg.api_base, "https://my-reg.example.com");
        assert_eq!(
            reg.index_base.as_deref(),
            Some("https://index.my-reg.example.com")
        );
        assert!(config.encryption.enabled);
        assert_eq!(config.encryption.passphrase.as_deref(), Some("my-pass"));
        assert_eq!(config.encryption.env_key.as_deref(), Some("MY_KEY"));
        assert_eq!(config.storage.bucket.as_deref(), Some("releases"));
        assert_eq!(config.storage.region.as_deref(), Some("eu-west-1"));
        assert_eq!(config.storage.base_path.as_deref(), Some("artifacts/"));
        assert!(config.validate().is_ok());
    }

    // ── Multi-registry TOML parsing ─────────────────────────────────

    #[test]
    fn parse_registries_section_toml() {
        let toml_str = r#"
[[registries.registries]]
name = "primary"
api_base = "https://primary.example.com"
default = true

[[registries.registries]]
name = "mirror"
api_base = "https://mirror.example.com"
index_base = "https://index.mirror.example.com"
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.registries.registries.len(), 2);
        assert_eq!(config.registries.registries[0].name, "primary");
        assert!(config.registries.registries[0].default);
        assert_eq!(config.registries.registries[1].name, "mirror");
        assert!(!config.registries.registries[1].default);
        assert!(config.validate().is_ok());
    }

    // ── Webhook TOML parsing ────────────────────────────────────────

    #[test]
    fn parse_webhook_section_toml() {
        let toml_str = r#"
[webhook]
url = "https://hooks.example.com/notify"
secret = "webhook-secret"
timeout_secs = 15
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.webhook.url, "https://hooks.example.com/notify");
        assert_eq!(config.webhook.secret.as_deref(), Some("webhook-secret"));
        assert_eq!(config.webhook.timeout_secs, 15);
    }

    // ── Verify / verify_timeout / verify_poll defaults ──────────────

    #[test]
    fn verify_timeout_defaults_when_not_set() {
        let config = ShipperConfig::default();
        let opts = config.build_runtime_options(CliOverrides::default());
        assert_eq!(opts.verify_timeout, Duration::from_secs(120));
        assert_eq!(opts.verify_poll_interval, Duration::from_secs(5));
    }

    #[test]
    fn verify_timeout_cli_override() {
        let config = ShipperConfig::default();
        let cli = CliOverrides {
            verify_timeout: Some(Duration::from_secs(300)),
            verify_poll_interval: Some(Duration::from_secs(10)),
            ..Default::default()
        };
        let opts = config.build_runtime_options(cli);
        assert_eq!(opts.verify_timeout, Duration::from_secs(300));
        assert_eq!(opts.verify_poll_interval, Duration::from_secs(10));
    }

    // ── Validation combinatorics ────────────────────────────────────

    #[test]
    fn validate_passes_with_jitter_at_exact_boundaries() {
        let mut config = ShipperConfig::default();
        config.retry.jitter = 0.0;
        config.readiness.jitter_factor = 0.0;
        assert!(config.validate().is_ok());

        config.retry.jitter = 1.0;
        config.readiness.jitter_factor = 1.0;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_zero_lock_timeout() {
        let mut config = ShipperConfig::default();
        config.lock.timeout = Duration::ZERO;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("lock.timeout"));
    }

    #[test]
    fn validate_rejects_equal_base_and_max_delay_only_when_base_exceeds() {
        let mut config = ShipperConfig::default();
        // base == max is OK
        config.retry.base_delay = Duration::from_secs(10);
        config.retry.max_delay = Duration::from_secs(10);
        assert!(config.validate().is_ok());

        // base > max is NOT OK
        config.retry.base_delay = Duration::from_secs(11);
        assert!(config.validate().is_err());
    }

    // ── Default TOML template ───────────────────────────────────────

    #[test]
    fn default_toml_template_roundtrips() {
        let template = ShipperConfig::default_toml_template();
        // Template should be parseable
        let parsed: ShipperConfig = toml::from_str(&template).unwrap();
        assert!(parsed.validate().is_ok());
    }

    // ── Serialization roundtrip ─────────────────────────────────────

    #[test]
    fn serialize_deserialize_roundtrip_preserves_all_fields() {
        let config = ShipperConfig {
            schema_version: "shipper.config.v1".to_string(),
            policy: PolicyConfig {
                mode: PublishPolicy::Fast,
            },
            verify: VerifyConfig {
                mode: VerifyMode::None,
            },
            readiness: ReadinessConfig {
                enabled: false,
                method: ReadinessMethod::Index,
                initial_delay: Duration::from_secs(7),
                max_delay: Duration::from_secs(45),
                max_total_wait: Duration::from_secs(180),
                poll_interval: Duration::from_secs(3),
                jitter_factor: 0.8,
                index_path: None,
                prefer_index: false,
            },
            output: OutputConfig { lines: 42 },
            lock: LockConfig {
                timeout: Duration::from_secs(900),
            },
            retry: RetryConfig {
                policy: RetryPolicy::Conservative,
                max_attempts: 2,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(60),
                strategy: RetryStrategyType::Linear,
                jitter: 0.15,
                per_error: PerErrorConfig::default(),
            },
            flags: FlagsConfig {
                allow_dirty: true,
                skip_ownership_check: true,
                strict_ownership: false,
            },
            parallel: ParallelConfig {
                enabled: true,
                max_concurrent: 12,
                per_package_timeout: Duration::from_secs(600),
            },
            state_dir: Some(PathBuf::from("custom-state")),
            registry: None,
            registries: MultiRegistryConfig::default(),
            webhook: WebhookConfig::default(),
            encryption: EncryptionConfigInner::default(),
            storage: StorageConfigInner::default(),
        };

        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: ShipperConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.policy.mode, PublishPolicy::Fast);
        assert_eq!(deserialized.verify.mode, VerifyMode::None);
        assert!(!deserialized.readiness.enabled);
        assert_eq!(deserialized.readiness.method, ReadinessMethod::Index);
        assert_eq!(deserialized.output.lines, 42);
        assert_eq!(deserialized.lock.timeout, Duration::from_secs(900));
        assert_eq!(deserialized.retry.policy, RetryPolicy::Conservative);
        assert_eq!(deserialized.retry.max_attempts, 2);
        assert!(deserialized.flags.allow_dirty);
        assert!(deserialized.flags.skip_ownership_check);
        assert!(deserialized.parallel.enabled);
        assert_eq!(deserialized.parallel.max_concurrent, 12);
        assert_eq!(
            deserialized.state_dir.as_ref().unwrap(),
            &PathBuf::from("custom-state")
        );
    }

    // ── Duration parsing edge cases ─────────────────────────────────

    #[test]
    fn duration_various_formats() {
        let toml_str = r#"
[lock]
timeout = "500ms"
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.lock.timeout, Duration::from_millis(500));

        let toml_str2 = r#"
[retry]
base_delay = "100ms"
max_delay = "1h"
"#;
        let config2: ShipperConfig = toml::from_str(toml_str2).unwrap();
        assert_eq!(config2.retry.base_delay, Duration::from_millis(100));
        assert_eq!(config2.retry.max_delay, Duration::from_secs(3600));
    }

    // ── Verify mode all variants via TOML ───────────────────────────

    #[test]
    fn verify_mode_workspace() {
        let config: ShipperConfig = toml::from_str("[verify]\nmode = \"workspace\"").unwrap();
        assert_eq!(config.verify.mode, VerifyMode::Workspace);
    }

    #[test]
    fn verify_mode_package() {
        let config: ShipperConfig = toml::from_str("[verify]\nmode = \"package\"").unwrap();
        assert_eq!(config.verify.mode, VerifyMode::Package);
    }

    #[test]
    fn verify_mode_none() {
        let config: ShipperConfig = toml::from_str("[verify]\nmode = \"none\"").unwrap();
        assert_eq!(config.verify.mode, VerifyMode::None);
    }

    // ── Retry strategy all variants via TOML ────────────────────────

    #[test]
    fn retry_strategy_immediate() {
        let config: ShipperConfig = toml::from_str("[retry]\nstrategy = \"immediate\"").unwrap();
        assert_eq!(config.retry.strategy, RetryStrategyType::Immediate);
    }

    #[test]
    fn retry_strategy_exponential() {
        let config: ShipperConfig = toml::from_str("[retry]\nstrategy = \"exponential\"").unwrap();
        assert_eq!(config.retry.strategy, RetryStrategyType::Exponential);
    }

    #[test]
    fn retry_strategy_linear() {
        let config: ShipperConfig = toml::from_str("[retry]\nstrategy = \"linear\"").unwrap();
        assert_eq!(config.retry.strategy, RetryStrategyType::Linear);
    }

    #[test]
    fn retry_strategy_constant() {
        let config: ShipperConfig = toml::from_str("[retry]\nstrategy = \"constant\"").unwrap();
        assert_eq!(config.retry.strategy, RetryStrategyType::Constant);
    }

    // ── Readiness method all variants via TOML ──────────────────────

    #[test]
    fn readiness_method_api() {
        let config: ShipperConfig = toml::from_str("[readiness]\nmethod = \"api\"").unwrap();
        assert_eq!(config.readiness.method, ReadinessMethod::Api);
    }

    #[test]
    fn readiness_method_index() {
        let config: ShipperConfig = toml::from_str("[readiness]\nmethod = \"index\"").unwrap();
        assert_eq!(config.readiness.method, ReadinessMethod::Index);
    }

    #[test]
    fn readiness_method_both() {
        let config: ShipperConfig = toml::from_str("[readiness]\nmethod = \"both\"").unwrap();
        assert_eq!(config.readiness.method, ReadinessMethod::Both);
    }

    // ── StorageConfigInner edge cases ───────────────────────────────

    #[test]
    fn storage_to_cloud_config_includes_all_optional_fields() {
        let toml_str = r#"
[storage]
storage_type = "S3"
bucket = "my-bucket"
region = "us-west-2"
base_path = "releases/v1/"
endpoint = "https://minio.local:9000"
access_key_id = "AKID"
secret_access_key = "SECRET"
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        let cloud = config.storage.to_cloud_config().unwrap();
        assert_eq!(cloud.bucket, "my-bucket");
        assert_eq!(cloud.region.as_deref(), Some("us-west-2"));
        assert_eq!(cloud.base_path, "releases/v1/");
        assert_eq!(cloud.endpoint.as_deref(), Some("https://minio.local:9000"));
        assert_eq!(cloud.access_key_id.as_deref(), Some("AKID"));
        assert_eq!(cloud.secret_access_key.as_deref(), Some("SECRET"));
    }

    #[test]
    fn storage_to_cloud_config_returns_none_without_bucket() {
        // Default storage has no bucket
        let config = ShipperConfig::default();
        assert!(config.storage.to_cloud_config().is_none());
    }

    #[test]
    fn storage_file_type_with_bucket_returns_cloud_config_but_not_configured() {
        let toml_str = r#"
[storage]
storage_type = "File"
bucket = "bucket"
"#;
        let config: ShipperConfig = toml::from_str(toml_str).unwrap();
        // is_configured checks both bucket.is_some() AND storage_type != File
        assert!(!config.storage.is_configured());
        // But to_cloud_config only checks bucket presence
        assert!(config.storage.to_cloud_config().is_some());
    }

    // ── MultiRegistryConfig edge cases ──────────────────────────────

    #[test]
    fn multi_registry_get_default_with_no_explicit_default_uses_first() {
        let cfg = MultiRegistryConfig {
            registries: vec![
                RegistryConfig {
                    name: "alpha".to_string(),
                    api_base: "https://alpha.example.com".to_string(),
                    index_base: None,
                    token: None,
                    default: false,
                },
                RegistryConfig {
                    name: "beta".to_string(),
                    api_base: "https://beta.example.com".to_string(),
                    index_base: None,
                    token: None,
                    default: false,
                },
            ],
            default_registries: vec![],
        };
        let default = cfg.get_default();
        assert_eq!(default.name, "alpha");
    }

    #[test]
    fn multi_registry_get_default_empty_returns_crates_io() {
        let cfg = MultiRegistryConfig::default();
        let default = cfg.get_default();
        assert_eq!(default.name, "crates-io");
        assert_eq!(default.api_base, "https://crates.io");
    }

    // ── Config merging with partial overrides ───────────────────────

    #[test]
    fn partial_cli_overrides_preserve_remaining_config_values() {
        let config = ShipperConfig {
            policy: PolicyConfig {
                mode: PublishPolicy::Balanced,
            },
            verify: VerifyConfig {
                mode: VerifyMode::Package,
            },
            retry: RetryConfig {
                policy: RetryPolicy::Custom,
                max_attempts: 15,
                base_delay: Duration::from_secs(3),
                max_delay: Duration::from_secs(180),
                strategy: RetryStrategyType::Exponential,
                jitter: 0.6,
                per_error: PerErrorConfig::default(),
            },
            output: OutputConfig { lines: 200 },
            lock: LockConfig {
                timeout: Duration::from_secs(1800),
            },
            flags: FlagsConfig {
                allow_dirty: true,
                skip_ownership_check: false,
                strict_ownership: true,
            },
            ..ShipperConfig::default()
        };

        // Only override policy and max_attempts
        let cli = CliOverrides {
            policy: Some(PublishPolicy::Fast),
            max_attempts: Some(2),
            ..Default::default()
        };

        let opts = config.build_runtime_options(cli);
        // Overridden values
        assert_eq!(opts.policy, PublishPolicy::Fast);
        assert_eq!(opts.max_attempts, 2);
        // Preserved config values
        assert_eq!(opts.verify_mode, VerifyMode::Package);
        assert_eq!(opts.base_delay, Duration::from_secs(3));
        assert_eq!(opts.max_delay, Duration::from_secs(180));
        assert_eq!(opts.output_lines, 200);
        assert_eq!(opts.lock_timeout, Duration::from_secs(1800));
        assert!(opts.allow_dirty);
        assert!(!opts.skip_ownership_check);
        assert!(opts.strict_ownership);
    }

    // ── into_runtime_options conversion ─────────────────────────────

    #[test]
    fn into_runtime_options_preserves_all_fields() {
        let config = ShipperConfig {
            policy: PolicyConfig {
                mode: PublishPolicy::Balanced,
            },
            ..ShipperConfig::default()
        };
        let runtime_opts = config.build_runtime_options(CliOverrides::default());
        let converted = into_runtime_options(runtime_opts);
        assert_eq!(converted.policy, PublishPolicy::Balanced);
        assert_eq!(converted.verify_mode, VerifyMode::Workspace);
        assert!(!converted.allow_dirty);
    }
}
