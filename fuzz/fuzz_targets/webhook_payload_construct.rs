#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_webhook::{publish_failure_payload, publish_success_payload, WebhookPayload};

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Split the input into 3 parts for use as package/version/registry|error
    let parts: Vec<&str> = input.splitn(3, '\0').collect();
    let (package, version, third) = match parts.len() {
        3 => (parts[0], parts[1], parts[2]),
        2 => (parts[0], parts[1], ""),
        _ => (input, "", ""),
    };

    // Construct success payload with arbitrary strings — must never panic
    let success = publish_success_payload(package, version, third);
    assert!(success.success);

    // Serialize the constructed payload — must always succeed
    let json = serde_json::to_vec(&success).expect("serialize success payload");
    let roundtrip: WebhookPayload =
        serde_json::from_slice(&json).expect("roundtrip success payload");
    assert_eq!(roundtrip.success, success.success);

    // Construct failure payload with arbitrary strings — must never panic
    let failure = publish_failure_payload(package, version, third);
    assert!(!failure.success);

    // Serialize the constructed payload — must always succeed
    let json = serde_json::to_vec(&failure).expect("serialize failure payload");
    let roundtrip: WebhookPayload =
        serde_json::from_slice(&json).expect("roundtrip failure payload");
    assert_eq!(roundtrip.success, failure.success);
});
