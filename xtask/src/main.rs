//! Internal tooling entry point for the `cargo xtask` alias.
//!
//! This crate is intentionally non-publishable (`publish = false`). It hosts
//! workspace-wide policy commands that need to run from a real Rust process —
//! beginning with package-surface and non-Rust file inventory commands which
//! feed the policy checker ladder.
//!
//! See `docs/policy/NON_RUST_ROLLOUT.md` for the full ladder.

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

mod authority_exceptions;
mod check_file_policy;
mod checks;
mod ci_labels;
mod clippy_checks;
mod doc_contracts;
mod file_policy;
mod mutants;
mod no_panic;
mod package_surface;
mod policy_report;
mod precommit;
mod propose;
mod release_identity;
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

    /// Report the workspace package surface from `cargo metadata`.
    #[command(name = "package-surface")]
    PackageSurface,

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

    /// Validate the approved release source, package graph, and release notes.
    #[command(name = "release-identity")]
    ReleaseIdentity(Box<release_identity::ReleaseIdentityArgs>),

    /// Validate Clippy lint policy: MSRV alignment + workspace.lints coverage.
    #[command(name = "check-lint-policy")]
    CheckLintPolicy,

    /// Validate `policy/clippy-exceptions.toml` schema, expiry, and bare-allow scan.
    #[command(name = "check-clippy-exceptions")]
    CheckClippyExceptions,

    /// Validate source-of-truth document contracts.
    #[command(name = "check-doc-contracts")]
    CheckDocContracts(DocContractsArgs),

    /// Validate or synchronize the source-owned opt-in CI label taxonomy.
    #[command(subcommand, name = "ci-labels")]
    CiLabels(CiLabelsCommand),

    /// Run or manage the repository-owned local pre-commit gate.
    #[command(name = "precommit")]
    Precommit(PrecommitArgs),

    /// Run the advisory ripr lane (`ripr pilot --root .`) — #182.
    ///
    /// Thin wrapper around the external `ripr` CLI. Shipper consumes
    /// ripr as an advisory PR lane; this command does not implement
    /// RIPR analysis itself. See docs/ci/ripr.md.
    #[command(name = "ripr-pr")]
    RiprPr(ripr::Args),

    /// Regenerate repo-scoped Shields badge endpoints (#182 PR 2).
    ///
    /// Runs `ripr check --root . --mode ready --format
    /// repo-exposure-json`, extracts `metrics.headline_eligible`, and
    /// writes `badges/ripr.json` and `badges/ripr-plus.json` as
    /// Shields-compatible endpoint JSON. README badges link these via
    /// raw.githubusercontent.com. Unlike `ripr-pr`, this command
    /// REQUIRES `ripr` on PATH — the badge artifacts are repo-truth and
    /// regenerating them with stale numbers is worse than failing loud.
    #[command(name = "repo-ripr-badge-artifacts")]
    RepoRiprBadgeArtifacts,

    /// Run `cargo mutants` against PR-changed files only (#182 PR 3).
    ///
    /// Label-gated PR-time mutation lane. Scopes mutation testing to
    /// files modified vs `--base` so it stays off the default PR hot
    /// path. Whole-workspace mutation remains in the weekly schedule.
    #[command(name = "mutants-pr")]
    MutantsPr(mutants::Args),
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

#[derive(Args, Debug)]
struct DocContractsArgs {
    /// Reporting / enforcement mode.
    #[arg(long, value_enum, default_value_t = doc_contracts::Mode::Advisory)]
    mode: doc_contracts::Mode,
}

#[derive(Args, Debug)]
struct PrecommitArgs {
    /// Hook lifecycle action. With no action, validates the staged index.
    #[command(subcommand)]
    action: Option<PrecommitAction>,
}

#[derive(Subcommand, Debug)]
enum PrecommitAction {
    /// Validate the staged index and branch-local Changie disposition.
    Run,
    /// Install the repository-owned pre-commit dispatcher.
    Install,
    /// Report whether the effective hook is current.
    Status,
    /// Remove only a repository-owned current or stale hook.
    Uninstall,
}

#[derive(Subcommand, Debug)]
enum CiLabelsCommand {
    /// Validate the manifest and workflow contract without network access.
    Check,
    /// Compare configured metadata to live GitHub labels without mutation.
    #[command(name = "check-live")]
    CheckLive(CiLabelsRepoArgs),
    /// Create/update configured labels; requires the explicit --apply flag.
    Sync(CiLabelsSyncArgs),
}

#[derive(Args, Debug)]
struct CiLabelsSyncArgs {
    /// Repository whose labels may be synchronized; must match policy.
    #[arg(long)]
    repo: String,
    /// Confirm the bounded live GitHub label mutation.
    #[arg(long)]
    apply: bool,
}

#[derive(Args, Debug)]
struct CiLabelsRepoArgs {
    /// Repository to compare; must match the source-owned policy.
    #[arg(long)]
    repo: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::NonRust(cmd) => match cmd {
            NonRustCommand::Inventory => file_policy::inventory()?,
            NonRustCommand::Propose => propose::propose()?,
        },
        Command::PackageSurface => package_surface::package_surface()?,
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
        Command::ReleaseIdentity(args) => release_identity::validate(*args)?,
        Command::CheckLintPolicy => clippy_checks::check_lint_policy()?,
        Command::CheckClippyExceptions => clippy_checks::check_clippy_exceptions()?,
        Command::CheckDocContracts(args) => doc_contracts::check(args.mode)?,
        Command::CiLabels(command) => match command {
            CiLabelsCommand::Check => ci_labels::check()?,
            CiLabelsCommand::CheckLive(args) => ci_labels::check_live(&args.repo)?,
            CiLabelsCommand::Sync(args) => ci_labels::sync(&args.repo, args.apply)?,
        },
        Command::Precommit(args) => match args.action.unwrap_or(PrecommitAction::Run) {
            PrecommitAction::Run => precommit::run()?,
            PrecommitAction::Install => precommit::install()?,
            PrecommitAction::Status => precommit::status()?,
            PrecommitAction::Uninstall => precommit::uninstall()?,
        },
        Command::RiprPr(args) => ripr::ripr_pr(&args)?,
        Command::RepoRiprBadgeArtifacts => ripr::repo_badge_artifacts()?,
        Command::MutantsPr(args) => mutants::mutants_pr(&args)?,
    }
    Ok(())
}
