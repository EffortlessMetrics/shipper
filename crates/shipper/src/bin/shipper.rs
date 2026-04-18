//! The `shipper` binary — thin wrapper over [`shipper_cli::run`].
//!
//! Keep this file small. All command-line logic lives in the
//! `shipper-cli` crate; all engine logic lives in `shipper-core`. The
//! `shipper` package exists as the install surface: a maintainer
//! types `cargo install shipper --locked` and gets a binary named
//! `shipper` that forwards here.
fn main() -> anyhow::Result<()> {
    shipper_cli::run()
}
