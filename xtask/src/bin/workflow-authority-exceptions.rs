//! Validate the exact, owned workflow-authority exception ledger.
//!
//! This binary is the schema/expiry ratchet for #267. The ledger model itself
//! lives in `xtask/src/authority_exceptions.rs` and is shared verbatim with
//! `check-workflow-surfaces --mode blocking-allowlist`, which reconciles these
//! exact identities against the detector's emitted findings. Keeping one model
//! keeps the ratchet and the blocking gate from drifting apart.

use anyhow::Result;

#[path = "../authority_exceptions.rs"]
mod authority_exceptions;

fn main() -> Result<()> {
    let root = authority_exceptions::workspace_root()?;
    let doc = authority_exceptions::load(&root)?;
    let today = authority_exceptions::current_utc_date()?;
    let count =
        authority_exceptions::validate_doc(&doc, today, |workflow| root.join(workflow).is_file())?;
    println!(
        "workflow authority exceptions: schema={} entries={} invalid=0 expired=0 duplicates=0",
        doc.schema_version, count
    );
    Ok(())
}
