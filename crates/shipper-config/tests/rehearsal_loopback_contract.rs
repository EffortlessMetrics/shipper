use shipper_config::{MultiRegistryConfig, RegistryConfig, RehearsalConfig, ShipperConfig};

fn loopback_registry(name: &str, port: u16) -> RegistryConfig {
    RegistryConfig {
        name: name.to_string(),
        api_base: format!("http://127.0.0.1:{port}/api"),
        index_base: Some(format!("http://127.0.0.1:{port}/index")),
        token: None,
        default: false,
        allow_private: false,
    }
}

fn distinct_loopback_config(catalog_live: bool) -> ShipperConfig {
    let live = loopback_registry("live", 18080);
    let rehearsal = loopback_registry("rehearsal", 18081);
    ShipperConfig {
        registry: (!catalog_live).then(|| live.clone()),
        registries: MultiRegistryConfig {
            registries: if catalog_live {
                vec![live, rehearsal]
            } else {
                vec![rehearsal]
            },
            default_registries: vec!["live".to_string()],
        },
        rehearsal: RehearsalConfig {
            enabled: true,
            allow_loopback: false,
            registry: Some("rehearsal".to_string()),
        },
        ..ShipperConfig::default()
    }
}

#[test]
fn explicit_loopback_permission_accepts_distinct_live_and_rehearsal_authorities() {
    for catalog_live in [false, true] {
        let config = distinct_loopback_config(catalog_live);
        // Both roles must be loopback here: a crates.io live target would not
        // detect loss of the public validator's explicit loopback permission.
        let result = config.validate_with_loopback(true);
        assert!(
            result.is_ok(),
            "explicit loopback permission must reach both distinct authorities \
             (catalog_live={catalog_live}): {result:?}"
        );
    }
}

#[test]
fn rehearsal_gate_does_not_implicitly_permit_a_loopback_live_target() {
    for catalog_live in [false, true] {
        let config = distinct_loopback_config(catalog_live);
        let error = config
            .validate_with_loopback(false)
            .expect_err("the live loopback target still needs explicit permission");
        let diagnostic = format!("{error:#}");
        assert!(
            diagnostic.contains("invalid registry 'live'"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("explicit loopback"), "{diagnostic}");
    }
}

#[test]
fn explicit_loopback_permission_does_not_allow_a_shared_authority() {
    let mut config = distinct_loopback_config(false);
    config.registries.registries[0] = loopback_registry("rehearsal", 18080);
    let error = config
        .validate_with_loopback(true)
        .expect_err("different names cannot isolate the same loopback authority");
    assert!(
        error
            .to_string()
            .contains("authority overlaps the live target")
    );
}

#[test]
fn explicit_loopback_permission_does_not_allow_the_live_registry_name() {
    let mut config = distinct_loopback_config(false);
    config.rehearsal.registry = Some("live".to_string());
    let error = config
        .validate_with_loopback(true)
        .expect_err("rehearsal cannot select the live registry name");
    assert!(
        error
            .to_string()
            .contains("must differ from the live target")
    );
}
