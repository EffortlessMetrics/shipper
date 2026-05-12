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
mod checks;
mod clippy_checks;
mod file_policy;
mod no_panic;
mod policy_report;
mod propose;
mod ripr;
mod workflow_checks;

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

    /// No-panic baseline + checker commands (#187).
    #[command(subcommand, name = "no-panic")]
    NoPanic(NoPanicCommand),

    /// Reconcile tracked non-Rust files against `policy/non-rust-allowlist.toml`.
    #[command(name = "check-file-policy")]
    CheckFilePolicy(CheckFilePolicyArgs),

    /// Validate `policy/generated-allowlist.toml` entries.
    #[command(name = "check-generated")]
    CheckGenerated(ChecksModeArgs),

    /// Reconcile tracked executable files against `policy/executable-allowlist.toml`.
    #[command(name = "check-executable-files")]
    CheckExecutableFiles(ChecksModeArgs),

    /// Reconcile dependency-manifest files against `policy/dependency-surface-allowlist.toml`.
    #[command(name = "check-dependency-surfaces")]
    CheckDependencySurfaces(ChecksModeArgs),

    /// Reconcile `.github/workflows/*.yml` against `policy/workflow-allowlist.toml`.
    #[command(name = "check-workflow-surfaces")]
    CheckWorkflowSurfaces(WorkflowModeArgs),

    /// Scan workflow contents for commands not in their declared process profile.
    #[command(name = "check-process-policy")]
    CheckProcessPolicy(WorkflowModeArgs),

    /// Scan workflow contents for endpoints not in their declared network profile.
    #[command(name = "check-network-policy")]
    CheckNetworkPolicy(WorkflowModeArgs),

    /// Run every advisory check and emit a unified policy report.
    #[command(name = "policy-report")]
    PolicyReport,

    /// Validate Clippy lint policy: MSRV alignment + workspace.lints coverage.
    #[command(name = "check-lint-policy")]
    CheckLintPolicy,

    /// Validate `policy/clippy-exceptions.toml` schema, expiry, and bare-allow scan.
    #[command(name = "check-clippy-exceptions")]
    CheckClippyExceptions,

    /// Run the advisory ripr lane (`ripr pilot --root .`) — #182.
    ///
    /// Thin wrapper around the external `ripr` CLI. Shipper consumes
    /// ripr as an advisory PR lane; this command does not implement
    /// RIPR analysis itself. See docs/ci/ripr.md.
    #[command(name = "ripr-pr")]
    RiprPr,
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

#[derive(Subcommand, Debug)]
enum NoPanicCommand {
    /// Regenerate `policy/no-panic-baseline.json` from current source.
    ///
    /// Walks `crates/*/src/**/*.rs` (excluding tests/benches/examples and
    /// `#[cfg(test)]`/`#[test]` subtrees), classifies every panic-family
    /// call site via syn, and writes the count-keyed baseline.
    Baseline,

    /// Verify that no new panic-family debt has been added since the
    /// baseline. Compares a fresh scan against `policy/no-panic-baseline.json`
    /// and exits non-zero (blocking) on any new entry or count increase.
    /// Resolved entries / count decreases are reported but do not fail.
    Check(NoPanicCheckArgs),
}

#[derive(Args, Debug)]
struct NoPanicCheckArgs {
    /// Reporting / enforcement mode.
    #[arg(long, value_enum, default_value_t = no_panic::Mode::Blocking)]
    mode: no_panic::Mode,
}

#[derive(Args, Debug)]
struct CheckFilePolicyArgs {
    /// Reporting / enforcement mode.
    #[arg(long, value_enum, default_value_t = check_file_policy::Mode::Advisory)]
    mode: check_file_policy::Mode,
}

#[derive(Args, Debug)]
struct ChecksModeArgs {
    /// Reporting / enforcement mode.
    #[arg(long, value_enum, default_value_t = checks::Mode::Advisory)]
    mode: checks::Mode,
}

#[derive(Args, Debug)]
struct WorkflowModeArgs {
    /// Reporting / enforcement mode.
    #[arg(long, value_enum, default_value_t = workflow_checks::Mode::Advisory)]
    mode: workflow_checks::Mode,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::NonRust(cmd) => match cmd {
            NonRustCommand::Inventory => file_policy::inventory()?,
            NonRustCommand::Propose => propose::propose()?,
        },
        Command::NoPanic(cmd) => match cmd {
            NoPanicCommand::Baseline => no_panic::baseline()?,
            NoPanicCommand::Check(args) => no_panic::check(args.mode)?,
        },
        Command::CheckFilePolicy(args) => check_file_policy::check(args.mode)?,
        Command::CheckGenerated(args) => checks::check_generated(args.mode)?,
        Command::CheckExecutableFiles(args) => checks::check_executable_files(args.mode)?,
        Command::CheckDependencySurfaces(args) => checks::check_dependency_surfaces(args.mode)?,
        Command::CheckWorkflowSurfaces(args) => {
            workflow_checks::check_workflow_surfaces(args.mode)?
        }
        Command::CheckProcessPolicy(args) => workflow_checks::check_process_policy(args.mode)?,
        Command::CheckNetworkPolicy(args) => workflow_checks::check_network_policy(args.mode)?,
        Command::PolicyReport => policy_report::policy_report()?,
        Command::CheckLintPolicy => clippy_checks::check_lint_policy()?,
        Command::CheckClippyExceptions => clippy_checks::check_clippy_exceptions()?,
        Command::RiprPr => ripr::ripr_pr()?,
    }
    Ok(())
}
