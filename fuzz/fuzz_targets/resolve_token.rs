#![no_main]

use std::fs;

use libfuzzer_sys::fuzz_target;
use shipper::auth::resolve_token;
use tempfile::tempdir;

fuzz_target!(|data: &[u8]| {
    let td = match tempdir() {
        Ok(v) => v,
        Err(_) => return,
    };

    let credentials = td.path().join("credentials.toml");
    if fs::write(&credentials, data).is_err() {
        return;
    }

    unsafe { std::env::set_var("CARGO_HOME", td.path()) };
    unsafe { std::env::remove_var("CARGO_REGISTRY_TOKEN") };
    unsafe { std::env::remove_var("CARGO_REGISTRIES_CRATES_IO_TOKEN") };

    let _ = resolve_token("crates-io");
});
