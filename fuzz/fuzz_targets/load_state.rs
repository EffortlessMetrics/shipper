#![no_main]

use std::fs;

use libfuzzer_sys::fuzz_target;
use shipper::state::load_state;
use tempfile::tempdir;

fuzz_target!(|data: &[u8]| {
    let td = match tempdir() {
        Ok(v) => v,
        Err(_) => return,
    };

    let path = td.path().join("state.json");
    if fs::write(path, data).is_ok() {
        let _ = load_state(td.path());
    }
});
