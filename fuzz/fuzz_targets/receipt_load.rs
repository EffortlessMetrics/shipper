#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper::types::Receipt;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Receipt>(data);
});
