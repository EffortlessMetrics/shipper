#![no_main]

use std::time::Duration;

use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize};
use shipper_duration::{deserialize_duration, parse_duration, serialize_duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurationWrapper {
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    value: Duration,
}

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_duration(s);

        let quoted = match serde_json::to_string(s) {
            Ok(v) => v,
            Err(_) => return,
        };
        let json = format!(r#"{{"value":{quoted}}}"#);
        if let Ok(wrapper) = serde_json::from_str::<DurationWrapper>(&json) {
            let _ = serde_json::to_vec(&wrapper);
        }
    }

    if data.len() >= 8 {
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&data[..8]);
        let millis = u64::from_le_bytes(bytes);
        let json = format!(r#"{{"value":{millis}}}"#);
        if let Ok(wrapper) = serde_json::from_str::<DurationWrapper>(&json) {
            assert_eq!(wrapper.value, Duration::from_millis(millis));
        }
    }
});
