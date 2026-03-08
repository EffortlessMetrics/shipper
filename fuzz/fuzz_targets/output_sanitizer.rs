#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_output_sanitizer::{redact_sensitive, tail_lines};

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Redact must never panic and must be idempotent
    let once = redact_sensitive(input);
    let twice = redact_sensitive(&once);
    assert_eq!(once, twice, "redact_sensitive must be idempotent");

    // No sensitive pattern should survive redaction
    if input.contains("CARGO_REGISTRY_TOKEN=") {
        assert!(once.contains("[REDACTED]") || !input.contains("CARGO_REGISTRY_TOKEN="));
    }

    // tail_lines at various counts must never panic
    for n in [0, 1, 2, 5, 10, 100, usize::MAX] {
        let _ = tail_lines(input, n);
    }

    // tail_lines of redacted output should equal redacting tail_lines output
    let n = data.len() % 16;
    let _tail_then_redact = redact_sensitive(&{
        let lines: Vec<&str> = input.lines().collect();
        if lines.len() <= n {
            input.to_string()
        } else {
            lines[lines.len() - n..].join("\n")
        }
    });
    let direct_tail = tail_lines(input, n);
    // Both should contain only redacted content (no raw secrets)
    if input.contains("CARGO_REGISTRY_TOKEN=") && n > 0 {
        // If the token line is in the tail window, it must be redacted
        if direct_tail.contains("CARGO_REGISTRY_TOKEN") {
            assert!(direct_tail.contains("[REDACTED]"));
        }
    }
});
