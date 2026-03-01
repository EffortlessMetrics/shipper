#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_duration::parse_duration;

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // parse_duration must never panic on any input
    if let Ok(d) = parse_duration(input) {
        // A successfully parsed duration formatted and re-parsed should roundtrip
        let formatted = humantime::format_duration(d).to_string();
        let reparsed =
            parse_duration(&formatted).expect("formatted duration must re-parse");
        assert_eq!(reparsed, d, "roundtrip failed for {input:?} → {formatted:?}");
    }

    // Try substrings — parsing should never panic
    for i in 0..input.len().min(32) {
        let _ = parse_duration(&input[i..]);
        let _ = parse_duration(&input[..input.len() - i]);
    }
});
