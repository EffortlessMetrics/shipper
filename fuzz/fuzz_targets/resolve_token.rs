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

    temp_env::with_vars(
        [
            (
                "CARGO_HOME",
                Some(td.path().to_str().unwrap_or_default()),
            ),
            ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
        ],
        || {
            let _ = resolve_token("crates-io");
        },
    );
});
