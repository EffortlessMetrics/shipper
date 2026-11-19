//! Signal policy for the executable entry point, never the embedding adapter.

use std::sync::OnceLock;

use anyhow::{Context, Result};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();

pub(super) fn install() -> Result<()> {
    let result = INSTALLED.get_or_init(|| {
        install_platform(shipper_core::engine::PublishInterruption::process_wide())
    });
    result
        .as_ref()
        .map_err(|message| anyhow::anyhow!(message.clone()))
        .context("no publish command was started")?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn install_platform(interruption: shipper_core::engine::PublishInterruption) -> Result<(), String> {
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        // Order matters: outside supervision the first action terminates
        // normally. During supervision it returns and the second requests
        // bounded cleanup. Both remain installed for process lifetime.
        signal_hook::flag::register_conditional_default(signal, interruption.default_action_flag())
            .and_then(|_| signal_hook::flag::register(signal, interruption.request_flag()))
            .map_err(|error| format!("cannot install publish termination handling: {error}"))?;
    }
    Ok(())
}

#[cfg(windows)]
fn install_platform(interruption: shipper_core::engine::PublishInterruption) -> Result<(), String> {
    use std::sync::atomic::Ordering::SeqCst;

    let requested = interruption.request_flag();
    let default_action = interruption.default_action_flag();
    ctrlc::set_handler(move || {
        // Publish the sticky request first. If the final lease has just been
        // released, the caller has already completed its durable decision.
        requested.store(true, SeqCst);
        if default_action.load(SeqCst) {
            std::process::exit(0xc000_013a_u32 as i32); // STATUS_CONTROL_C_EXIT
        }
    })
    .map_err(|error| format!("cannot install publish termination handling: {error}"))
}
