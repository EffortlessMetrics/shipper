#![no_main]

use libfuzzer_sys::fuzz_target;

use shipper_output_sanitizer::{redact_sensitive, tail_lines};

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(input) => input,
        Err(_) => return,
    };

    let sanitized = redact_sensitive(input);
    assert_eq!(redact_sensitive(&sanitized), sanitized);

    let tail_n = input.len() % 8;
    let tail = tail_lines(input, tail_n);
    let sanitized_tail = tail_lines(&sanitized, tail_n);
    assert_eq!(tail, sanitized_tail);
});
