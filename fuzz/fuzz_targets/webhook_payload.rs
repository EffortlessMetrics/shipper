#![no_main]

use libfuzzer_sys::fuzz_target;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use shipper_webhook::{WebhookConfig, WebhookPayload};

fuzz_target!(|data: &[u8]| {
    // 1. WebhookPayload deserialization from arbitrary bytes
    if let Ok(payload) = serde_json::from_slice::<WebhookPayload>(data) {
        // Roundtrip: serialize then deserialize again must succeed
        let json = serde_json::to_vec(&payload).expect("serialize back");
        let _: WebhookPayload =
            serde_json::from_slice(&json).expect("roundtrip deserialize");
    }

    // 2. WebhookConfig deserialization from arbitrary bytes
    if let Ok(config) = serde_json::from_slice::<WebhookConfig>(data) {
        let json = serde_json::to_vec(&config).expect("serialize back");
        let _: WebhookConfig =
            serde_json::from_slice(&json).expect("roundtrip deserialize");
    }

    // 3. HMAC signature computation with arbitrary inputs
    if let Ok(text) = std::str::from_utf8(data) {
        // Split the input in half: first half as secret, second half as body
        let mid = text.len() / 2;
        let (secret, body) = text.split_at(mid);
        if !secret.is_empty() {
            if let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
                mac.update(body.as_bytes());
                let digest = mac.finalize().into_bytes();
                // Verify the digest has the expected length (32 bytes for SHA-256)
                assert_eq!(digest.len(), 32);
            }
        }
    }

    // 4. URL parsing for webhook endpoints
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = url::Url::parse(text);
    }
});
