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
use clap::{Args, Parser, Subcommand};

mod check_file_policy;
mod file_policy;
mod propose;

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

    /// Reconcile tracked non-Rust files against `policy/non-rust-allowlist.toml`.
    #[command(name = "check-file-policy")]
    CheckFilePolicy(CheckFilePolicyArgs),
}

#[derive(Subcommand, Debug)]
enum NonRustCommand {
    /// Inventory all tracked non-Rust files in the workspace.
    ///
    /// Emits a Markdown summary and a JSON payload to `target/policy/`.
    /// The output is consumed by `check-file-policy`.
    Inventory,

    /// Propose draft allowlist entries for unreceipted non-Rust files.
    ///
    /// Writes `target/policy/non-rust-proposed-allowlist.toml` and
    /// `non-rust-proposal.md`. Never mutates the real ledger.
    Propose,
}

#[derive(Args, Debug)]
struct CheckFilePolicyArgs {
    /// Reporting / enforcement mode.
    #[arg(long, value_enum, default_value_t = check_file_policy::Mode::Advisory)]
    mode: check_file_policy::Mode,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::NonRust(cmd) => match cmd {
            NonRustCommand::Inventory => file_policy::inventory()?,
            NonRustCommand::Propose => propose::propose()?,
        },
        Command::CheckFilePolicy(args) => check_file_policy::check(args.mode)?,
    }
    Ok(())
}
