use std::path::Path;

use tempfile::tempdir;
use shipper_types::PublishPolicy;

use shipper_config::ShipperConfig;

#[test]
fn default_toml_template_is_loadable_via_file_api() {
    let td = tempdir().expect("tempdir");
    let path = td.path().join(".shipper.toml");

    std::fs::write(&path, ShipperConfig::default_toml_template())
        .expect("write config template");

    let loaded = ShipperConfig::load_from_file(Path::new(&path)).expect("load template");
    assert_eq!(loaded.retry.max_attempts, 6);
    assert_eq!(loaded.output.lines, 50);
    assert_eq!(loaded.policy.mode, PublishPolicy::Safe);
}

#[test]
fn build_runtime_options_can_merge_cli_and_file_defaults_without_panic() {
    let config = ShipperConfig::default();
    let overrides = shipper_config::CliOverrides {
        output_lines: Some(99),
        ..Default::default()
    };

    let options = config.build_runtime_options(overrides);
    assert_eq!(options.output_lines, 99);
    assert!(!options.no_verify);
}
