#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_config::ShipperConfig;

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Parse arbitrary TOML into ShipperConfig, then validate if successful
    if let Ok(config) = toml::from_str::<ShipperConfig>(input) {
        // Validate must not panic on any successfully-parsed config
        let _ = config.validate();

        // Roundtrip: serialize back to TOML and re-parse
        if let Ok(serialized) = toml::to_string(&config) {
            let _ = toml::from_str::<ShipperConfig>(&serialized);
        }
    }

    // Also try deserializing from JSON (exercises serde from a different format)
    if let Ok(config) = serde_json::from_str::<ShipperConfig>(input) {
        let _ = config.validate();
    }
});
