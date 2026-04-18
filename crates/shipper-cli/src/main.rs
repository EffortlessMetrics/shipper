//! `shipper-cli` is a compatibility shim (#95).
//!
//! The real CLI lives in the `shipper` crate's `cli` module. This
//! binary exists so that operators who `cargo install shipper-cli` on
//! the old name keep getting a working CLI during the migration
//! window. Prefer `cargo install shipper --locked` on new setups.
fn main() -> anyhow::Result<()> {
    shipper::cli::run()
}
