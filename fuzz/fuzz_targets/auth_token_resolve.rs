#![no_main]

use std::fs;

use libfuzzer_sys::fuzz_target;
use shipper_core::auth::resolve_token;
use tempfile::tempdir;

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Split into registry name and credentials content
    let (registry_name, cred_content) = match input.split_once('\0') {
        Some((name, content)) => (name, content),
        None => (input, ""),
    };

    // Skip empty registry names (would be useless)
    if registry_name.is_empty() || registry_name.len() > 128 {
        return;
    }

    let td = match tempdir() {
        Ok(v) => v,
        Err(_) => return,
    };

    let credentials = td.path().join("credentials.toml");
    if fs::write(&credentials, cred_content).is_err() {
        return;
    }

    // Resolve token with an arbitrary registry name and credentials content
    temp_env::with_vars(
        [
            ("CARGO_HOME", Some(td.path().to_str().unwrap_or_default())),
            ("CARGO_REGISTRY_TOKEN", None::<&str>),
            ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<&str>),
        ],
        || {
            let _ = resolve_token(registry_name);
        },
    );
});
