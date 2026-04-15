#![no_main]

use std::time::Duration;

use libfuzzer_sys::fuzz_target;
use shipper_config::{
    CliOverrides, PublishPolicy, ReadinessMethod, Registry, ShipperConfig, VerifyMode,
};
use shipper_config::runtime::into_runtime_options;

fn next_u8(data: &[u8], idx: &mut usize) -> u8 {
    let value = *data.get(*idx).unwrap_or(&0);
    if *idx < data.len() {
        *idx += 1;
    }
    value
}

fn next_u64(data: &[u8], idx: &mut usize) -> u64 {
    let start = *idx;
    let end = (*idx + 8).min(data.len());
    *idx = end;
    let mut bytes = [0u8; 8];
    bytes[..(end - start)].copy_from_slice(&data[start..end]);
    u64::from_le_bytes(bytes)
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let mut idx = 0usize;
    let base_config = ShipperConfig::default();
    let mut source = base_config.build_runtime_options(CliOverrides::default());

    let policy = match next_u8(data, &mut idx) % 3 {
        0 => PublishPolicy::Safe,
        1 => PublishPolicy::Balanced,
        _ => PublishPolicy::Fast,
    };
    let verify_mode = match next_u8(data, &mut idx) % 3 {
        0 => VerifyMode::Workspace,
        1 => VerifyMode::Package,
        _ => VerifyMode::None,
    };
    let readiness_method = match next_u8(data, &mut idx) % 3 {
        0 => ReadinessMethod::Api,
        1 => ReadinessMethod::Index,
        _ => ReadinessMethod::Both,
    };

    source.policy = policy;
    source.verify_mode = verify_mode;
    source.readiness.enabled = true;
    source.readiness.method = readiness_method;
    source.max_attempts = (next_u64(data, &mut idx) % 500 + 1) as u32;
    source.base_delay = Duration::from_millis(next_u64(data, &mut idx) % 60_000);
    source.max_delay = source
        .base_delay
        .max(Duration::from_millis(next_u64(data, &mut idx) % 120_000));
    source.output_lines = (next_u64(data, &mut idx) % 2000 + 1) as usize;
    source.webhook.url = if (next_u8(data, &mut idx) % 2) == 0 {
        String::from("https://fuzz.example/webhook")
    } else {
        String::new()
    };
    source.webhook.secret = if (next_u8(data, &mut idx) % 2) == 0 {
        Some("secret".to_string())
    } else {
        None
    };
    source.webhook.timeout_secs = 5 + next_u64(data, &mut idx) % 300;
    source.encryption.enabled = (next_u8(data, &mut idx) % 2) == 1;
    source.encryption.passphrase = if source.encryption.enabled {
        Some("pass".to_string())
    } else {
        None
    };
    source.encryption.env_var = if (next_u8(data, &mut idx) % 2) == 1 {
        Some("SHIPPER_ENCRYPT_KEY".to_string())
    } else {
        None
    };
    source.parallel.enabled = (next_u8(data, &mut idx) % 2) == 1;
    source.parallel.max_concurrent = (next_u8(data, &mut idx) % 16) as usize + 1;
    source.parallel.per_package_timeout =
        Duration::from_secs(120 + (next_u64(data, &mut idx) % 600));
    source.registries = (0..((next_u8(data, &mut idx) % 4) as usize))
        .map(|idx| Registry {
            name: format!("registry-{idx}"),
            api_base: format!("https://{idx}.example"),
            index_base: Some(format!("https://index.{idx}.example")),
        })
        .collect();
    source.force_resume = (next_u8(data, &mut idx) % 2) == 1;
    source.strict_ownership = (next_u8(data, &mut idx) % 2) == 1;
    source.skip_ownership_check = (next_u8(data, &mut idx) % 2) == 1;
    source.allow_dirty = (next_u8(data, &mut idx) % 2) == 1;

    let converted = into_runtime_options(source.clone());

    assert_eq!(converted.policy, source.policy);
    assert_eq!(converted.verify_mode, source.verify_mode);
    assert_eq!(converted.readiness.enabled, source.readiness.enabled);
    assert_eq!(converted.readiness.method, source.readiness.method);
    assert_eq!(converted.max_attempts, source.max_attempts);
    assert_eq!(converted.output_lines, source.output_lines);
    assert_eq!(converted.webhook.url, source.webhook.url);
    assert_eq!(converted.webhook.secret, source.webhook.secret);
    assert_eq!(converted.webhook.timeout_secs, source.webhook.timeout_secs);
    assert_eq!(converted.force_resume, source.force_resume);
    assert_eq!(
        converted.parallel.max_concurrent,
        source.parallel.max_concurrent
    );
    assert_eq!(converted.registries.len(), source.registries.len());
});
