#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_cargo_failure::classify_publish_failure;

fuzz_target!(|data: (Vec<u8>, Vec<u8>)| {
    let (stderr_bytes, stdout_bytes) = data;
    let stderr = String::from_utf8_lossy(&stderr_bytes);
    let stdout = String::from_utf8_lossy(&stdout_bytes);

    // Core safety invariant: classifier should be deterministic and never panic.
    let first = classify_publish_failure(&stderr, &stdout);
    let second = classify_publish_failure(&stderr, &stdout);
    assert_eq!(first, second);
});
