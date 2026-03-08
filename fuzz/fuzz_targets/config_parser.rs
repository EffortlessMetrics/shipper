#![no_main]

use std::fs;
use std::path::Path;
use std::str;

use libfuzzer_sys::fuzz_target;
use tempfile::tempdir;

fuzz_target!(|data: &[u8]| {
    let input = match str::from_utf8(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    let dir = match tempdir() {
        Ok(v) => v,
        Err(_) => return,
    };

    let path = dir.path().join(".shipper.toml");
    if fs::write(&path, input).is_err() {
        return;
    }

    let config = shipper_config::ShipperConfig::load_from_file(Path::new(&path));
    if let Ok(config) = config {
        let _ = config.validate();
        let _ = config.build_runtime_options(shipper_config::CliOverrides::default());
    }
});
