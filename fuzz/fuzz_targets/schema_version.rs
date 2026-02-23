#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    let _ = shipper_schema::parse_schema_version(input);
    let _ = shipper_schema::validate_schema_version(input, "shipper.receipt.v1", "schema");
});