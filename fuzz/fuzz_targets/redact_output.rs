#![no_main]

use libfuzzer_sys::fuzz_target;

use shipper_output_sanitizer::{redact_sensitive, tail_lines};

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(input) => input,
        Err(_) => return,
    };

    let sanitized = redact_sensitive(input);
    // Idempotence: redacting already-redacted text should be a no-op
    assert_eq!(redact_sensitive(&sanitized), sanitized);

    // tail_lines should not panic for any input
    let tail_n = input.len() % 8;
    let _ = tail_lines(input, tail_n);
    let _ = tail_lines(&sanitized, tail_n);
});
