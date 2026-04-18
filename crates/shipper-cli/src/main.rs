//! The `shipper-cli` binary — thin wrapper over [`shipper_cli::run`].
//!
//! `shipper-cli` is a real CLI adapter (not a shim). Operators should
//! prefer `cargo install shipper --locked` — the `shipper` package on
//! crates.io installs a binary named `shipper` that forwards to this
//! same [`run`]. This binary exists for two reasons:
//!
//! 1. Backward compatibility for anyone with `cargo install shipper-cli`
//!    wired into their pipelines on the old name.
//! 2. Local workspace development: `cargo run -p shipper-cli -- <args>`
//!    is still a reasonable way to exercise the CLI without installing.
fn main() -> anyhow::Result<()> {
    shipper_cli::run()
}
