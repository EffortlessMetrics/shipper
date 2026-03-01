#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_config::ShipperConfig;

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    let _ = toml::from_str::<ShipperConfig>(input);
});
