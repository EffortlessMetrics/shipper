//! Internal tooling entry point for the `cargo xtask` alias.
//!
//! This crate is intentionally non-publishable (`publish = false`). It hosts
//! workspace-wide policy commands that need to run from a real Rust process —
//! beginning with the non-Rust file inventory (`cargo xtask non-rust
//! inventory`) which feeds the file-policy checker that lands in later
//! rollout PRs.
//!
//! See `docs/policy/NON_RUST_ROLLOUT.md` for the full ladder.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod file_policy;

#[derive(Parser, Debug)]
#[command(
    name = "xtask",
    about = "Internal tooling for the shipper workspace",
    disable_help_subcommand = true,
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Non-Rust file policy commands.
    #[command(subcommand, name = "non-rust")]
    NonRust(NonRustCommand),
}

#[derive(Subcommand, Debug)]
enum NonRustCommand {
    /// Inventory all tracked non-Rust files in the workspace.
    ///
    /// Emits a Markdown summary and a JSON payload to `target/policy/`.
    /// The output is consumed by later policy checkers
    /// (`cargo xtask check-file-policy`, landing in a follow-up PR).
    Inventory,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::NonRust(cmd) => match cmd {
            NonRustCommand::Inventory => file_policy::inventory()?,
        },
    }
    Ok(())
}
