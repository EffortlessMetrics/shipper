//! The `shipper` binary — thin shim over [`shipper::cli::run`] (#95).
fn main() -> anyhow::Result<()> {
    shipper::cli::run()
}
