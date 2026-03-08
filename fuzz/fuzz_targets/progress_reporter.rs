#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_progress::ProgressReporter;

use std::str;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }

    let total = (data[0] as usize % 16) + 1;
    let index = (data[1] as usize % total) + 1;
    let midpoint = (data.len() - 1) / 2 + 1;
    let name = str::from_utf8(&data[1..midpoint]).unwrap_or("demo");
    let version = str::from_utf8(&data[midpoint..]).unwrap_or("1.0.0");

    let mut reporter = ProgressReporter::silent(total);
    reporter.set_package(index, name, version);
    reporter.set_status("fuzzing");
    reporter.finish_package();
    reporter.finish();
});
