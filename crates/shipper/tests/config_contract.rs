use tempfile::tempdir;

#[test]
fn shipper_config_module_uses_same_schema_contract_as_config_crate() {
    let td = tempdir().expect("tempdir");
    let config_path = td.path().join(".shipper.toml");

    let template = shipper::config::ShipperConfig::default_toml_template();
    std::fs::write(&config_path, template).expect("write template");

    let loaded = shipper::config::ShipperConfig::load_from_file(&config_path)
        .expect("load config");
    let merged = loaded.build_runtime_options(shipper::config::CliOverrides {
        output_lines: Some(128),
        ..Default::default()
    });

    assert_eq!(loaded.retry.max_attempts, 6);
    assert_eq!(merged.output_lines, 128);
}
