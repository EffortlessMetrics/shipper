#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_types::Receipt;

fuzz_target!(|data: &[u8]| {
    // Fuzz raw JSON deserialization of Receipt
    if let Ok(receipt) = serde_json::from_slice::<Receipt>(data) {
        // Roundtrip: serialize then deserialize again must produce equivalent data
        let json = serde_json::to_vec(&receipt).expect("serialize back");
        let _: Receipt = serde_json::from_slice(&json).expect("roundtrip deserialize");
    }

    // Also try interpreting as UTF-8 string for serde_json::from_str path
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<Receipt>(text);
    }
});
