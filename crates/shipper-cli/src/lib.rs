//! # shipper-cli
//!
//! Real CLI adapter for Shipper (#95 three-crate split).
//!
//! This crate owns the command-line surface: argument parsing
//! (`clap`), subcommand dispatch, help text, progress rendering. It
//! depends on [`shipper_core`] for the actual engine.
//!
//! ## Architecture
//!
//! ```text
//! shipper (install façade) -> shipper-cli (this crate) -> shipper-core (engine)
//! ```
//!
//! The `shipper` binary on crates.io is a three-line wrapper that
//! calls [`run`]; a separate `shipper-cli` binary exists in this
//! crate for backward compatibility with `cargo install shipper-cli`
//! and for workspace-local development.
//!
//! ## Embedding
//!
//! Most callers should use the `shipper` CLI directly. If you need to
//! embed the exact CLI surface in another Rust program — for example,
//! a wrapper that invokes `shipper` with extra preflight steps — call
//! [`run`]. For programmatic use without a `clap` dependency, depend
//! on [`shipper_core`](https://crates.io/crates/shipper-core) instead.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use shipper_core::config::{CliOverrides, ShipperConfig};
use shipper_core::engine::{self, Reporter};
use shipper_core::plan;
use shipper_core::types::{Finishability, PreflightReport, Registry, ReleaseSpec, RuntimeOptions};

mod output;

use crate::output::progress::ProgressReporter;

#[derive(Parser, Debug)]
#[command(name = "shipper", version)]
#[command(about = "Resumable, backoff-aware crates.io publishing for workspaces")]
struct Cli {
    /// Path to a custom configuration file (.shipper.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Path to the workspace Cargo.toml
    #[arg(long, default_value = "Cargo.toml", global = true)]
    manifest_path: PathBuf,

    /// Cargo registry name (default: crates-io)
    #[arg(long, global = true)]
    registry: Option<String>,

    /// Registry API base URL (default: <https://crates.io>)
    #[arg(long, global = true)]
    api_base: Option<String>,

    /// Restrict to specific packages (repeatable). If omitted, publishes all publishable workspace members.
    #[arg(long = "package", global = true)]
    packages: Vec<String>,

    /// Directory for shipper state and receipts (default: .shipper)
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

    /// Number of output lines to capture for evidence (default: 50)
    #[arg(long, global = true)]
    output_lines: Option<usize>,

    /// Allow publishing from a dirty git working tree.
    #[arg(long, global = true)]
    allow_dirty: bool,

    /// Skip owners/permissions preflight.
    #[arg(long, global = true)]
    skip_ownership_check: bool,

    /// Fail preflight if ownership checks fail or if no token is available.
    ///
    /// Note: crates.io token scopes may not allow querying owners; this is best-effort.
    #[arg(long, global = true)]
    strict_ownership: bool,

    /// Pass --no-verify to cargo publish.
    #[arg(long, global = true)]
    no_verify: bool,

    /// Max attempts per crate publish step (default: 6)
    #[arg(long, global = true)]
    max_attempts: Option<u32>,

    /// Base backoff delay (e.g. 2s, 500ms; default: 2s)
    #[arg(long, global = true)]
    base_delay: Option<String>,

    /// Max backoff delay (e.g. 2m; default: 2m)
    #[arg(long, global = true)]
    max_delay: Option<String>,

    /// Retry strategy: immediate, exponential (default), linear, constant
    #[arg(long, global = true)]
    retry_strategy: Option<String>,

    /// Jitter factor for retry delays (0.0 = no jitter, 1.0 = full jitter; default: 0.5)
    #[arg(long, global = true)]
    retry_jitter: Option<f64>,

    /// How long to wait for registry visibility after a successful publish (default: 2m)
    #[arg(long, global = true)]
    verify_timeout: Option<String>,

    /// Poll interval for checking registry visibility (default: 5s)
    #[arg(long, global = true)]
    verify_poll: Option<String>,

    /// Readiness check method: api (default, fast), index (slower, more accurate), both (slowest, most reliable)
    #[arg(long, global = true)]
    readiness_method: Option<String>,

    /// How long to wait for registry visibility during readiness checks (default: 5m)
    #[arg(long, global = true)]
    readiness_timeout: Option<String>,

    /// Poll interval for readiness checks (default: 2s)
    #[arg(long, global = true)]
    readiness_poll: Option<String>,

    /// Disable readiness checks (for advanced users).
    #[arg(long, global = true)]
    no_readiness: bool,

    /// Force resume even if the computed plan differs from the state file.
    #[arg(long, global = true)]
    force_resume: bool,

    /// Force override of existing locks (use with caution)
    #[arg(long, global = true)]
    force: bool,

    /// Lock timeout duration (e.g. 1h, 30m; default: 1h). Locks older than this are considered stale.
    #[arg(long, global = true)]
    lock_timeout: Option<String>,

    /// Publish policy: safe (verify+strict), balanced (verify when needed), fast (no verify; default: safe)
    #[arg(long, global = true)]
    policy: Option<String>,

    /// Verify mode: workspace (default), package (per-crate), none (no verify)
    #[arg(long, global = true)]
    verify_mode: Option<String>,

    /// Enable parallel publishing (packages at the same dependency level are published concurrently)
    #[arg(long, global = true)]
    parallel: bool,

    /// Maximum number of concurrent publish operations (implies --parallel)
    #[arg(long, global = true)]
    max_concurrent: Option<usize>,

    /// Timeout per package publish operation when using parallel mode (e.g. 30m, 1h)
    #[arg(long, global = true)]
    per_package_timeout: Option<String>,

    /// Webhook URL to send publish event notifications to
    #[arg(long, global = true)]
    webhook_url: Option<String>,

    /// Optional secret for signing webhook payloads
    #[arg(long, global = true)]
    webhook_secret: Option<String>,

    /// Enable encryption for state files
    #[arg(long, global = true)]
    encrypt: bool,

    /// Passphrase for state file encryption (or use SHIPPER_ENCRYPT_KEY env var)
    #[arg(long, global = true)]
    encrypt_passphrase: Option<String>,

    /// Target registries for multi-registry publishing (comma-separated list)
    /// Example: --registries crates-io,my-registry
    #[arg(long, global = true)]
    registries: Option<String>,

    /// Publish to all configured registries
    #[arg(long, global = true)]
    all_registries: bool,

    /// Optional package name to resume from
    #[arg(long, global = true)]
    resume_from: Option<String>,

    /// Name of a registry (from `[[registries]]` in `.shipper.toml`) to
    /// rehearse the publish against before live dispatch.
    ///
    /// See issue #97. Plumbed through today; phase-2 execution (actual
    /// publish to the rehearsal registry + install/smoke checks + live
    /// dispatch gate) lands in a follow-on PR.
    #[arg(long, global = true)]
    rehearsal_registry: Option<String>,

    /// Skip rehearsal even if `.shipper.toml` enables it.
    ///
    /// Use with caution — rehearsal (once fully implemented under #97)
    /// is the proof boundary between "we built it" and "we verified it
    /// actually resolves from a registry." Bypassing it should be rare.
    #[arg(long, global = true)]
    skip_rehearsal: bool,

    /// Crate name to smoke-install after a successful rehearsal (#97 PR 4).
    ///
    /// Runs `cargo install --registry <rehearsal> <CRATE>` against the
    /// rehearsal registry to prove the crate actually resolves and
    /// installs end-to-end — the scenario that workspace-path
    /// dependencies defeat and that killed the rc.1 first-publish.
    ///
    /// The named crate must be in the plan AND have a `[[bin]]` target.
    /// Library-only crates cannot be smoke-installed directly; use a
    /// consumer-workspace build instead (follow-on).
    #[arg(long = "smoke-install", global = true, value_name = "CRATE")]
    rehearsal_smoke_install: Option<String>,

    /// Output format: text (default) or json
    #[arg(long, default_value = "text", value_parser = ["text", "json"], global = true)]
    format: String,

    /// Show detailed dependency analysis for plan command
    #[arg(long, global = true)]
    verbose: bool,

    /// Suppress informational output
    #[arg(short, long, global = true)]
    quiet: bool,

    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print the deterministic publish plan (dependency-first ordering).
    Plan,
    /// Run preflight checks without publishing.
    Preflight,
    /// Execute the plan (will resume if a matching state file exists).
    Publish,
    /// Resume a previous publish run.
    Resume,
    /// Rehearse a release against an alternate registry (#97 PR 2).
    ///
    /// Publishes every crate in the plan to the registry named by
    /// `--rehearsal-registry` (or `[rehearsal] registry = "..."` in
    /// `.shipper.toml`), verifies visibility on that registry, and
    /// emits a `RehearsalComplete { passed, ... }` event to
    /// `events.jsonl` so the outcome is auditable.
    ///
    /// Rehearse must target a non-live registry (kellnr, a sandbox
    /// crates.io account, or a throwaway alternate registry). Shipper
    /// refuses to rehearse against the same registry as the live target.
    ///
    /// Part of [#97](https://github.com/EffortlessMetrics/shipper/issues/97).
    /// The hard gate that blocks live publish without a passing rehearsal
    /// lands in #97 PR 3.
    Rehearse,
    /// Compare local workspace versions to the registry.
    Status,
    /// Print environment and auth diagnostics.
    Doctor,
    /// View detailed event log.
    InspectEvents,
    /// View detailed receipt with evidence.
    InspectReceipt,
    /// Print CI configuration snippets for various platforms.
    #[command(subcommand)]
    Ci(CiCommands),
    /// Clean state files (state.json, receipt.json, events.jsonl).
    Clean {
        /// Keep receipt.json (only remove state.json and events.jsonl)
        #[arg(long)]
        keep_receipt: bool,
    },
    /// Yank a crate@version from the registry — containment, not undo.
    ///
    /// `cargo yank` marks a specific version as not-installable for NEW
    /// dependency resolves. Existing lockfile pins and already-downloaded
    /// copies are unaffected. See
    /// [cargo yank docs](https://doc.rust-lang.org/cargo/commands/cargo-yank.html).
    ///
    /// Part of [#98 Remediate](https://github.com/EffortlessMetrics/shipper/issues/98).
    /// Follow-on commands (`shipper plan-yank`, `shipper fix-forward`)
    /// compose this primitive into reverse-topological containment and
    /// fix-forward plans.
    Yank {
        /// Name of the crate to yank (e.g., `shipper-types`). Required
        /// unless `--plan` is supplied.
        #[arg(long = "crate", value_name = "NAME", conflicts_with = "plan")]
        crate_name: Option<String>,
        /// Version to yank (e.g., `1.2.3`). Required unless `--plan`
        /// is supplied.
        #[arg(long, value_name = "VERSION", conflicts_with = "plan")]
        version: Option<String>,
        /// Operator-supplied reason. Required unless `--plan` is supplied.
        /// Recorded in the event log, audit trails, and any future
        /// receipts that reference this yank.
        ///
        /// Example: `"CVE-2026-0001 disclosed; containing while patch
        /// released"`.
        #[arg(long, conflicts_with = "plan")]
        reason: Option<String>,
        /// Also mark the crate's existing receipt entry as compromised
        /// (#98 PR 3). Ignored in `--plan` mode (plan execution already
        /// carries per-entry reasons from the planning step).
        #[arg(long)]
        mark_compromised: bool,
        /// **Plan execution mode** (#98 PR 5). Path to a yank plan JSON
        /// file (the `--format json` output of `shipper plan-yank`).
        /// Walks the plan's entries in order, invoking `cargo yank` for
        /// each. Mutually exclusive with `--crate` / `--version` /
        /// `--reason`.
        #[arg(long, value_name = "PATH")]
        plan: Option<PathBuf>,
    },
    /// Generate a reverse-topological yank plan from a receipt (#98 PR 2).
    ///
    /// Reads a prior `receipt.json` and emits the order in which to yank
    /// the released crates — dependents first, dependencies last — so
    /// downstream consumers stop resolving against the bad version before
    /// the bad version itself is pulled. Output is either human-readable
    /// `shipper yank ...` lines or structured JSON for scripting.
    ///
    /// **Planning only.** This command does NOT execute yanks. Pipe the
    /// output through `sh`, or consume the JSON, once you've reviewed it.
    /// `shipper fix-forward` (#98 PR 3) will wrap execution.
    PlanYank {
        /// Path to the receipt to derive the plan from. Defaults to
        /// `<state_dir>/receipt.json` when omitted.
        #[arg(long, value_name = "PATH")]
        from_receipt: Option<PathBuf>,
        /// Restrict the plan to packages whose receipt carries a
        /// `compromised_at` marker. Without this, every `Published`
        /// package is included (full rollback). Mutually exclusive
        /// with `--starting-crate`.
        #[arg(long, conflicts_with = "starting_crate")]
        compromised_only: bool,
        /// **Graph mode** (#98 PR 4). Given a specific broken crate
        /// name, walk the workspace's dependency graph to find every
        /// crate that transitively depends on it, and emit a yank
        /// plan covering only that affected chain (not a full
        /// rollback). Resolves the graph from the current workspace's
        /// `Cargo.toml` metadata — the receipt supplies the versions
        /// and Published-state filter.
        #[arg(long, value_name = "CRATE")]
        starting_crate: Option<String>,
        /// Per-entry reason to embed in the yank plan (applied to
        /// every entry). If omitted, each entry's reason falls back
        /// to its receipt-level `compromised_by` field (if set).
        #[arg(long, value_name = "REASON")]
        reason: Option<String>,
    },
    /// Generate a fix-forward supersession plan from a compromised
    /// receipt (#98 PR 3).
    ///
    /// Reads a prior `receipt.json`, finds packages whose receipt entry
    /// carries a `compromised_at` marker (populated by
    /// `shipper yank ... --mark-compromised`), and prints an ordered
    /// list of successor versions to publish. Dependencies go first
    /// (opposite of plan-yank) so downstream consumers can upgrade to a
    /// clean chain on `cargo update`.
    ///
    /// **Planning only.** This command does NOT edit Cargo.toml or
    /// invoke publish — that's operator territory. It prints the
    /// steps, you execute them.
    #[command(name = "fix-forward")]
    FixForward {
        /// Path to the compromised receipt. Defaults to
        /// `<state_dir>/receipt.json` when omitted.
        #[arg(long, value_name = "PATH")]
        from_receipt: Option<PathBuf>,
    },
    /// Configuration file management.
    #[command(subcommand)]
    Config(ConfigCommands),
    /// Generate shell completion scripts for the specified shell.
    Completion {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand, Debug)]
enum CiCommands {
    /// Print GitHub Actions workflow snippet.
    #[command(name = "github-actions")]
    GitHubActions,
    /// Print GitLab CI workflow snippet.
    #[command(name = "gitlab")]
    GitLab,
    /// Print CircleCI workflow snippet.
    #[command(name = "circleci")]
    CircleCI,
    /// Print Azure DevOps pipeline snippet.
    #[command(name = "azure-devops")]
    AzureDevOps,
}

#[derive(Subcommand, Debug, Clone)]
enum ConfigCommands {
    /// Generate a default .shipper.toml configuration file.
    Init {
        /// Output path for the configuration file (default: .shipper.toml)
        #[arg(short, long, default_value = ".shipper.toml")]
        output: PathBuf,
    },
    /// Validate a configuration file.
    Validate {
        /// Path to the configuration file to validate (default: .shipper.toml)
        #[arg(short, long, default_value = ".shipper.toml")]
        path: PathBuf,
    },
}

struct CliReporter {
    quiet: bool,
}

impl Reporter for CliReporter {
    fn info(&mut self, msg: &str) {
        if !self.quiet {
            eprintln!("[info] {msg}");
        }
    }

    fn warn(&mut self, msg: &str) {
        if !self.quiet {
            eprintln!("[warn] {msg}");
        }
    }

    fn error(&mut self, msg: &str) {
        eprintln!("[error] {msg}");
    }
}

/// CLI entry point. Exposed for the `shipper` crate's binary target
/// and for the `shipper-cli` compatibility shim, both of which are
/// three-line `fn main() { shipper_cli::run() }` wrappers.
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    // Handle Config commands early (they don't need workspace plan)
    if let Commands::Config(config_cmd) = &cli.cmd {
        return run_config(config_cmd.clone());
    }

    // Handle Completion commands early (they don't need workspace plan)
    if let Commands::Completion { shell } = &cli.cmd {
        return run_completion(shell);
    }

    let api_base = cli
        .api_base
        .clone()
        .unwrap_or_else(|| "https://crates.io".to_string());
    let index_base = cli.api_base.as_ref().map(|_| api_base.clone());

    let spec = ReleaseSpec {
        manifest_path: cli.manifest_path.clone(),
        registry: Registry {
            name: cli
                .registry
                .clone()
                .unwrap_or_else(|| "crates-io".to_string()),
            api_base,
            index_base,
        },
        selected_packages: if cli.packages.is_empty() {
            None
        } else {
            Some(cli.packages.clone())
        },
    };

    let mut planned = plan::build_plan(&spec)?;

    // Load configuration file
    let config =
        if let Some(ref config_path) = cli.config {
            // Use custom config file specified via --config
            Some(ShipperConfig::load_from_file(config_path).with_context(|| {
                format!("Failed to load config from: {}", config_path.display())
            })?)
        } else {
            // Try to load .shipper.toml from workspace root
            ShipperConfig::load_from_workspace(&planned.workspace_root)
                .with_context(|| "Failed to load config from workspace")?
        };

    // Validate loaded configuration before using it for runtime options.
    if let Some(ref cfg) = config {
        let config_path = cli
            .config
            .clone()
            .unwrap_or_else(|| planned.workspace_root.join(".shipper.toml"));
        cfg.validate().with_context(|| {
            format!(
                "Configuration validation failed for {}",
                config_path.display()
            )
        })?;
    }

    // Apply registry from config if CLI didn't set it
    if let Some(ref cfg) = config
        && let Some(ref reg_config) = cfg.registry
    {
        if cli.registry.is_none() {
            planned.plan.registry.name = reg_config.name.clone();
        }
        if cli.api_base.is_none() {
            planned.plan.registry.api_base = reg_config.api_base.clone();
            planned.plan.registry.index_base = reg_config.index_base.clone();
        }
    }

    // Build CLI overrides
    let cli_overrides = CliOverrides {
        policy: cli.policy.as_deref().map(parse_policy).transpose()?,
        verify_mode: cli
            .verify_mode
            .as_deref()
            .map(parse_verify_mode)
            .transpose()?,
        max_attempts: cli.max_attempts,
        base_delay: cli.base_delay.as_deref().map(parse_duration).transpose()?,
        max_delay: cli.max_delay.as_deref().map(parse_duration).transpose()?,
        retry_strategy: cli
            .retry_strategy
            .as_deref()
            .map(parse_retry_strategy)
            .transpose()?,
        retry_jitter: cli.retry_jitter,
        verify_timeout: cli
            .verify_timeout
            .as_deref()
            .map(parse_duration)
            .transpose()?,
        verify_poll_interval: cli.verify_poll.as_deref().map(parse_duration).transpose()?,
        output_lines: cli.output_lines,
        lock_timeout: cli
            .lock_timeout
            .as_deref()
            .map(parse_duration)
            .transpose()?,
        state_dir: cli.state_dir.clone(),
        readiness_method: cli
            .readiness_method
            .as_deref()
            .map(parse_readiness_method)
            .transpose()?,
        readiness_timeout: cli
            .readiness_timeout
            .as_deref()
            .map(parse_duration)
            .transpose()?,
        readiness_poll: cli
            .readiness_poll
            .as_deref()
            .map(parse_duration)
            .transpose()?,
        allow_dirty: cli.allow_dirty,
        skip_ownership_check: cli.skip_ownership_check,
        strict_ownership: cli.strict_ownership,
        no_verify: cli.no_verify,
        no_readiness: cli.no_readiness,
        force: cli.force,
        force_resume: cli.force_resume,
        parallel_enabled: cli.parallel || cli.max_concurrent.is_some(),
        max_concurrent: cli.max_concurrent,
        per_package_timeout: cli
            .per_package_timeout
            .as_deref()
            .map(parse_duration)
            .transpose()?,
        webhook_url: cli.webhook_url.clone(),
        webhook_secret: cli.webhook_secret.clone(),
        encrypt: cli.encrypt,
        encrypt_passphrase: cli.encrypt_passphrase.clone(),
        registries: cli.registries.as_ref().map(|s| {
            s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        }),
        all_registries: cli.all_registries,
        resume_from: cli.resume_from.clone(),
        rehearsal_registry: cli.rehearsal_registry.clone(),
        skip_rehearsal: cli.skip_rehearsal,
        rehearsal_smoke_install: cli.rehearsal_smoke_install.clone(),
    };

    // Merge CLI overrides with config (or defaults if no config)
    let config_for_merge = config.clone().unwrap_or_default();
    let opts: RuntimeOptions = config_for_merge.build_runtime_options(cli_overrides);

    let mut reporter = CliReporter { quiet: cli.quiet };

    match cli.cmd {
        Commands::Plan => {
            print_plan(&planned, cli.verbose);
        }
        Commands::Preflight => {
            let rep = engine::run_preflight(&planned, &opts, &mut reporter)?;
            print_preflight(&rep, &cli.format);
        }
        Commands::Publish => {
            let target_registries = if opts.registries.is_empty() {
                vec![planned.plan.registry.clone()]
            } else {
                opts.registries.clone()
            };

            for reg in target_registries {
                if opts.registries.len() > 1 {
                    println!(
                        "\n🚀 Publishing to registry: {} ({})",
                        reg.name, reg.api_base
                    );
                }

                let mut current_planned = planned.clone();
                current_planned.plan.registry = reg.clone();

                let mut current_opts = opts.clone();
                // Segregate state dir by registry name if multiple registries
                if opts.registries.len() > 1 {
                    current_opts.state_dir = opts.state_dir.join(&reg.name);
                }

                let total_packages = current_planned.plan.packages.len();
                let mut progress = ProgressReporter::new(total_packages, cli.quiet);

                // Show initial progress if we have packages
                if total_packages > 0 {
                    let first_pkg = &current_planned.plan.packages[0];
                    progress.set_package(1, &first_pkg.name, &first_pkg.version);
                }

                let receipt = engine::run_publish(&current_planned, &current_opts, &mut reporter)?;

                progress.finish();

                print_receipt(
                    &receipt,
                    &current_planned.workspace_root,
                    &current_opts.state_dir,
                    &cli.format,
                );
            }
        }
        Commands::Resume => {
            let target_registries = if opts.registries.is_empty() {
                vec![planned.plan.registry.clone()]
            } else {
                opts.registries.clone()
            };

            for reg in target_registries {
                if opts.registries.len() > 1 {
                    println!(
                        "\n🔄 Resuming for registry: {} ({})",
                        reg.name, reg.api_base
                    );
                }

                let mut current_planned = planned.clone();
                current_planned.plan.registry = reg.clone();

                let mut current_opts = opts.clone();
                if opts.registries.len() > 1 {
                    current_opts.state_dir = opts.state_dir.join(&reg.name);
                }

                let total_packages = current_planned.plan.packages.len();
                let mut progress = ProgressReporter::new(total_packages, cli.quiet);

                // Show initial progress if we have packages
                if total_packages > 0 {
                    let first_pkg = &current_planned.plan.packages[0];
                    progress.set_package(1, &first_pkg.name, &first_pkg.version);
                }

                let receipt = engine::run_resume(&current_planned, &current_opts, &mut reporter)?;

                progress.finish();

                print_receipt(
                    &receipt,
                    &current_planned.workspace_root,
                    &current_opts.state_dir,
                    &cli.format,
                );
            }
        }
        Commands::Rehearse => {
            let outcome = engine::run_rehearsal(&planned, &opts, &mut reporter)?;

            // Stdout is the operator-facing receipt: mirrors the live
            // publish path, so a human scanning the terminal sees one
            // consistent "did it work?" line regardless of which command
            // they ran. Full per-package detail is in events.jsonl.
            if outcome.passed {
                println!(
                    "rehearsal OK: {} packages against '{}'",
                    outcome.packages_published, outcome.registry_name
                );
            } else {
                println!(
                    "rehearsal FAILED after {}/{} packages against '{}': {}",
                    outcome.packages_published,
                    outcome.packages_attempted,
                    outcome.registry_name,
                    outcome.summary
                );
                // Exit non-zero so CI lanes that wrap `shipper rehearse`
                // fail the job on a failed rehearsal without needing extra
                // scripting.
                anyhow::bail!("rehearsal did not pass");
            }
        }
        Commands::Status => {
            let target_registries = if opts.registries.is_empty() {
                vec![planned.plan.registry.clone()]
            } else {
                opts.registries.clone()
            };

            for reg in target_registries {
                if opts.registries.len() > 1 {
                    println!("\n📊 Status for registry: {} ({})", reg.name, reg.api_base);
                }
                let mut current_planned = planned.clone();
                current_planned.plan.registry = reg;
                run_status(&current_planned, &mut reporter)?;
            }
        }
        Commands::Doctor => {
            let target_registries = if opts.registries.is_empty() {
                vec![planned.plan.registry.clone()]
            } else {
                opts.registries.clone()
            };

            for reg in target_registries {
                if opts.registries.len() > 1 {
                    println!(
                        "\n🩺 Diagnostics for registry: {} ({})",
                        reg.name, reg.api_base
                    );
                }
                let mut current_planned = planned.clone();
                current_planned.plan.registry = reg;
                run_doctor(&current_planned, &opts, &mut reporter)?;
            }
        }
        Commands::InspectEvents => {
            run_inspect_events(&planned, &opts)?;
        }
        Commands::InspectReceipt => {
            run_inspect_receipt(&planned, &opts, &cli.format)?;
        }
        Commands::Ci(ci_cmd) => {
            run_ci(ci_cmd, &opts.state_dir, &planned.workspace_root)?;
        }
        Commands::Yank {
            crate_name,
            version,
            reason,
            mark_compromised,
            plan,
        } => {
            use shipper_core::cargo;
            use shipper_core::engine::plan_yank;
            use shipper_core::state::events::{EventLog, events_path};
            use shipper_core::state::execution_state::{load_receipt, receipt_path, write_receipt};
            use shipper_core::types::{EventType, PublishEvent};

            // #98 PR 5 — plan execution mode. Dispatched entirely
            // separately from the single-yank path below; the two share
            // the same cargo_yank primitive but different orchestration.
            if let Some(plan_path) = plan {
                let yank_plan = plan_yank::load_plan_from_path(&plan_path)?;
                reporter.info(&format!(
                    "executing yank plan: {} entries against '{}' (plan_id {})",
                    yank_plan.entries.len(),
                    yank_plan.registry,
                    yank_plan.plan_id
                ));

                let workspace_root = std::env::current_dir()
                    .context("failed to resolve current dir for plan execution")?;
                let registry_name = opts
                    .registries
                    .first()
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| yank_plan.registry.clone());

                let mut log = EventLog::new();
                let events_file = events_path(&opts.state_dir);

                let mut succeeded = 0usize;
                let mut failed: Option<(String, i32)> = None;

                for (i, entry) in yank_plan.entries.iter().enumerate() {
                    let entry_reason = entry
                        .reason
                        .clone()
                        .unwrap_or_else(|| "plan execution".to_string());
                    reporter.warn(&format!(
                        "[{}/{}] yanking {}@{} — reason: {}",
                        i + 1,
                        yank_plan.entries.len(),
                        entry.name,
                        entry.version,
                        entry_reason
                    ));

                    let out = cargo::cargo_yank(
                        &workspace_root,
                        entry.name.as_str(),
                        entry.version.as_str(),
                        registry_name.as_str(),
                        opts.output_lines,
                        None,
                    )?;

                    log.record(PublishEvent {
                        timestamp: chrono::Utc::now(),
                        event_type: EventType::PackageYanked {
                            crate_name: entry.name.clone(),
                            version: entry.version.clone(),
                            reason: entry_reason.clone(),
                            exit_code: out.exit_code,
                        },
                        package: format!("{}@{}", entry.name, entry.version),
                    });
                    if let Err(err) = log.write_to_file(&events_file) {
                        reporter.warn(&format!(
                            "failed to append PackageYanked event to {}: {err:#}",
                            events_file.display()
                        ));
                    }
                    log.clear();

                    if out.exit_code == 0 {
                        succeeded += 1;
                        reporter.info(&format!(
                            "[{}/{}] yanked {}@{}",
                            i + 1,
                            yank_plan.entries.len(),
                            entry.name,
                            entry.version
                        ));
                    } else {
                        reporter.error(&format!(
                            "[{}/{}] cargo yank exited {} for {}@{}. stderr tail:\n{}",
                            i + 1,
                            yank_plan.entries.len(),
                            out.exit_code,
                            entry.name,
                            entry.version,
                            out.stderr_tail
                        ));
                        failed = Some((format!("{}@{}", entry.name, entry.version), out.exit_code));
                        // Halt on first failure. Plan is reverse-topo so
                        // every entry below this one is a dependent of
                        // something we just failed to yank — continuing
                        // would only produce more damage.
                        break;
                    }
                }

                if let Some((pkg, code)) = failed {
                    reporter.error(&format!(
                        "yank plan halted: {succeeded}/{} succeeded; failed at {pkg} (cargo exit {code})",
                        yank_plan.entries.len()
                    ));
                    anyhow::bail!(
                        "yank plan failed at {pkg}; {succeeded}/{} entries succeeded before halt",
                        yank_plan.entries.len()
                    );
                } else {
                    reporter.info(&format!(
                        "yank plan complete: {succeeded}/{} entries yanked successfully",
                        yank_plan.entries.len()
                    ));
                    return Ok(());
                }
            }

            // Single-yank mode (the original shape). All three fields
            // are required when `--plan` is absent; clap's
            // `conflicts_with` already rejected the mixed combinations.
            let crate_name = crate_name.ok_or_else(|| {
                anyhow::anyhow!("--crate is required when --plan is not supplied")
            })?;
            let version = version.ok_or_else(|| {
                anyhow::anyhow!("--version is required when --plan is not supplied")
            })?;
            let reason = reason.ok_or_else(|| {
                anyhow::anyhow!("--reason is required when --plan is not supplied")
            })?;

            reporter.warn(&format!(
                "yanking {crate_name}@{version} from registry \
                 (containment, not undo) — reason: {reason}"
            ));

            let workspace_root =
                std::env::current_dir().context("failed to resolve current dir for cargo yank")?;
            let registry_name = opts
                .registries
                .first()
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "crates-io".to_string());

            let out = cargo::cargo_yank(
                &workspace_root,
                crate_name.as_str(),
                version.as_str(),
                registry_name.as_str(),
                opts.output_lines,
                None,
            )?;

            let mut log = EventLog::new();
            log.record(PublishEvent {
                timestamp: chrono::Utc::now(),
                event_type: EventType::PackageYanked {
                    crate_name: crate_name.clone(),
                    version: version.clone(),
                    reason: reason.clone(),
                    exit_code: out.exit_code,
                },
                package: format!("{crate_name}@{version}"),
            });
            let events_file = events_path(&opts.state_dir);
            if let Err(err) = log.write_to_file(&events_file) {
                reporter.warn(&format!(
                    "failed to append PackageYanked event to {}: {err:#}",
                    events_file.display()
                ));
            }

            if out.exit_code == 0 {
                if mark_compromised {
                    // #98 PR 3: mirror the yank into the receipt so
                    // downstream commands (plan-yank --compromised-only,
                    // fix-forward) can find the marker without scanning
                    // events.jsonl. The receipt is a *projection*, so
                    // mutating one field on one matching package is a
                    // legitimate amendment.
                    let rpath = receipt_path(&opts.state_dir);
                    match load_receipt(&opts.state_dir) {
                        Ok(Some(mut receipt)) => {
                            let matched = receipt
                                .packages
                                .iter_mut()
                                .find(|p| p.name == crate_name && p.version == version);
                            if let Some(pkg) = matched {
                                pkg.compromised_at = Some(chrono::Utc::now());
                                pkg.compromised_by = Some(reason.clone());
                                if let Err(err) = write_receipt(&opts.state_dir, &receipt) {
                                    reporter.warn(&format!(
                                        "yanked successfully but failed to mark receipt at \
                                         {}: {err:#}",
                                        rpath.display()
                                    ));
                                } else {
                                    reporter.info(&format!(
                                        "marked {crate_name}@{version} compromised in {}",
                                        rpath.display()
                                    ));
                                }
                            } else {
                                reporter.warn(&format!(
                                    "--mark-compromised: no matching package entry for \
                                     {crate_name}@{version} in {}; yank succeeded but the \
                                     receipt was not amended.",
                                    rpath.display()
                                ));
                            }
                        }
                        Ok(None) => {
                            reporter.warn(&format!(
                                "--mark-compromised: no receipt at {}; yank succeeded but \
                                 nothing to amend. Future plan-yank / fix-forward runs won't \
                                 see this version as compromised unless the receipt is \
                                 reconstructed.",
                                rpath.display()
                            ));
                        }
                        Err(err) => {
                            reporter.warn(&format!(
                                "--mark-compromised: failed to load receipt at {}: {err:#}. \
                                 Yank succeeded; receipt not amended.",
                                rpath.display()
                            ));
                        }
                    }
                }

                reporter.info(&format!(
                    "yanked {crate_name}@{version} successfully. \
                     existing lockfile pins are NOT invalidated; \
                     downstream consumers should `cargo update -p {crate_name}` \
                     to pick up the next available version."
                ));
            } else {
                reporter.error(&format!(
                    "cargo yank exited {} for {crate_name}@{version}. \
                     stderr tail:\n{}",
                    out.exit_code, out.stderr_tail
                ));
                anyhow::bail!(
                    "yank failed for {crate_name}@{version} (cargo exit {})",
                    out.exit_code
                );
            }
        }
        Commands::PlanYank {
            from_receipt,
            compromised_only,
            starting_crate,
            reason,
        } => {
            use shipper_core::engine::plan_yank::{self, PlanYankFilter};

            let receipt_path = from_receipt.unwrap_or_else(|| {
                opts.state_dir
                    .join(shipper_core::state::execution_state::RECEIPT_FILE)
            });

            let receipt = plan_yank::load_receipt_from_path(&receipt_path).with_context(|| {
                "plan-yank needs a readable receipt; default path is \
                 <state_dir>/receipt.json. Pass --from-receipt <path> to \
                 override."
                    .to_string()
            })?;

            // Three mutually-informative modes:
            //   --starting-crate <N>   → graph mode (walk dependents)
            //   --compromised-only     → receipt-filter mode (marker)
            //   (default)              → receipt-filter mode (all Published)
            // clap's `conflicts_with` already rejects combinations at parse time.
            let plan = if let Some(ref starting) = starting_crate {
                // Graph mode uses the *current workspace's* dependency graph,
                // read from the planned workspace we already built upstream.
                plan_yank::build_plan_from_starting_crate(
                    &receipt,
                    &planned.plan.dependencies,
                    starting,
                    reason.clone(),
                )?
            } else {
                let filter = if compromised_only {
                    PlanYankFilter::CompromisedOnly
                } else {
                    PlanYankFilter::AllPublished
                };
                plan_yank::build_plan(&receipt, filter)
            };

            match cli.format.as_str() {
                "json" => {
                    let out = serde_json::to_string_pretty(&plan)
                        .context("failed to serialize yank plan as JSON")?;
                    println!("{out}");
                }
                _ => {
                    println!("{}", plan_yank::render_text(&plan));
                }
            }
        }
        Commands::FixForward { from_receipt } => {
            use shipper_core::engine::fix_forward::{self, SuccessorStrategy};

            let receipt_path = from_receipt.unwrap_or_else(|| {
                opts.state_dir
                    .join(shipper_core::state::execution_state::RECEIPT_FILE)
            });

            let plan =
                fix_forward::plan_from_path(&receipt_path, SuccessorStrategy::PlaceholderNext)
                    .with_context(|| {
                        "fix-forward needs a readable receipt; default path is \
                         <state_dir>/receipt.json. Pass --from-receipt <path> to \
                         override."
                            .to_string()
                    })?;

            match cli.format.as_str() {
                "json" => {
                    let out = serde_json::to_string_pretty(&plan)
                        .context("failed to serialize fix-forward plan as JSON")?;
                    println!("{out}");
                }
                _ => {
                    println!("{}", fix_forward::render_text(&plan));
                }
            }
        }
        Commands::Clean { keep_receipt } => {
            run_clean(
                &opts.state_dir,
                &planned.workspace_root,
                keep_receipt,
                opts.force,
            )?;
        }
        Commands::Config(_) => {
            // This should never be reached since we handle Config commands early
            unreachable!("Config commands should be handled before this match");
        }
        Commands::Completion { .. } => {
            // This should never be reached since we handle Completion commands early
            unreachable!("Completion commands should be handled before this match");
        }
    }

    Ok(())
}

fn parse_duration(s: &str) -> Result<Duration> {
    shipper_duration::parse_duration(s).with_context(|| format!("invalid duration: {s}"))
}

fn parse_policy(s: &str) -> Result<shipper_core::config::PublishPolicy> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(shipper_core::config::PublishPolicy::Safe),
        "balanced" => Ok(shipper_core::config::PublishPolicy::Balanced),
        "fast" => Ok(shipper_core::config::PublishPolicy::Fast),
        _ => bail!("invalid policy: {s} (expected: safe, balanced, fast)"),
    }
}

fn parse_verify_mode(s: &str) -> Result<shipper_core::config::VerifyMode> {
    match s.to_lowercase().as_str() {
        "workspace" => Ok(shipper_core::config::VerifyMode::Workspace),
        "package" => Ok(shipper_core::config::VerifyMode::Package),
        "none" => Ok(shipper_core::config::VerifyMode::None),
        _ => bail!("invalid verify-mode: {s} (expected: workspace, package, none)"),
    }
}

fn parse_readiness_method(s: &str) -> Result<shipper_core::config::ReadinessMethod> {
    match s.to_lowercase().as_str() {
        "api" => Ok(shipper_core::config::ReadinessMethod::Api),
        "index" => Ok(shipper_core::config::ReadinessMethod::Index),
        "both" => Ok(shipper_core::config::ReadinessMethod::Both),
        _ => bail!("invalid readiness-method: {s} (expected: api, index, both)"),
    }
}

fn parse_retry_strategy(s: &str) -> Result<shipper_core::retry::RetryStrategyType> {
    match s.to_lowercase().as_str() {
        "immediate" => Ok(shipper_core::retry::RetryStrategyType::Immediate),
        "exponential" => Ok(shipper_core::retry::RetryStrategyType::Exponential),
        "linear" => Ok(shipper_core::retry::RetryStrategyType::Linear),
        "constant" => Ok(shipper_core::retry::RetryStrategyType::Constant),
        _ => bail!(
            "invalid retry-strategy: {s} (expected: immediate, exponential, linear, constant)"
        ),
    }
}

fn print_plan(ws: &plan::PlannedWorkspace, verbose: bool) {
    println!("plan_id: {}", ws.plan.plan_id);
    println!(
        "registry: {} ({})",
        ws.plan.registry.name, ws.plan.registry.api_base
    );
    println!("workspace_root: {}", ws.workspace_root.display());
    println!();

    let total_packages = ws.plan.packages.len();
    println!("Total packages to publish: {}", total_packages);
    println!();

    if !ws.skipped.is_empty() {
        println!("Skipped packages:");
        for p in &ws.skipped {
            println!("  - {}@{} ({})", p.name, p.version, p.reason);
        }
        println!();
    }

    if verbose {
        // Enhanced verbose output with dependency analysis
        print_detailed_plan(ws);
    } else {
        // Simple output
        for (idx, p) in ws.plan.packages.iter().enumerate() {
            println!("{:>3}. {}@{}", idx + 1, p.name, p.version);
        }
    }
}

fn print_detailed_plan(ws: &plan::PlannedWorkspace) {
    // Get dependency levels for parallel publishing analysis
    let levels = ws.plan.group_by_levels();
    let total_levels = levels.len();

    println!("=== Dependency Analysis ===");
    println!();

    // Show dependency levels for parallel publishing
    println!("Publishing Levels (packages at same level can be published in parallel):");
    println!();
    for level in &levels {
        let level_pkgs: Vec<String> = level
            .packages
            .iter()
            .map(|p| format!("{}@{}", p.name, p.version))
            .collect();
        println!("  Level {}: {}", level.level, level_pkgs.join(", "));
    }
    println!();

    // Show full dependency graph
    println!("Dependency Graph:");
    println!();
    for (idx, p) in ws.plan.packages.iter().enumerate() {
        let deps = ws.plan.dependencies.get(&p.name);
        let deps_str = match deps {
            Some(deps) if !deps.is_empty() => {
                let dep_versions: Vec<String> = deps
                    .iter()
                    .filter_map(|dep_name| {
                        ws.plan
                            .packages
                            .iter()
                            .find(|pkg| &pkg.name == dep_name)
                            .map(|pkg| format!("{}@{}", dep_name, pkg.version))
                    })
                    .collect();
                format!("depends on: {}", dep_versions.join(", "))
            }
            _ => String::from("no workspace dependencies"),
        };
        println!("  {:>3}. {}@{} ({})", idx + 1, p.name, p.version, deps_str);
    }
    println!();

    // Show potential issues / preflight considerations
    println!("=== Preflight Considerations ===");
    println!();

    // Analyze potential issues
    let mut issues: Vec<String> = Vec::new();

    // Check for packages with many dependencies (may take longer)
    for p in &ws.plan.packages {
        #[allow(clippy::collapsible_if)]
        if let Some(deps) = ws.plan.dependencies.get(&p.name) {
            if deps.len() > 3 {
                issues.push(format!(
                    "  - {}@{} has {} dependencies (may require longer publish time)",
                    p.name,
                    p.version,
                    deps.len()
                ));
            }
        }
    }

    // Check for packages that are depended upon by many others
    let mut dependents_count: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for deps in ws.plan.dependencies.values() {
        for dep in deps {
            *dependents_count.entry(dep.as_str()).or_insert(0) += 1;
        }
    }
    for (name, count) in &dependents_count {
        #[allow(clippy::collapsible_if)]
        if *count > 3 {
            if let Some(pkg) = ws.plan.packages.iter().find(|p| p.name == *name) {
                issues.push(format!(
                    "  - {}@{} is a core dependency for {} packages (critical path)",
                    pkg.name, pkg.version, count
                ));
            }
        }
    }

    if issues.is_empty() {
        println!("  No obvious issues detected.");
        println!("  All packages have reasonable dependency structures.");
    } else {
        for issue in &issues {
            println!("{}", issue);
        }
    }
    println!();

    // Estimate time analysis (rough estimates)
    println!("=== Estimated Publishing Analysis ===");
    println!();

    // Calculate max parallel packages per level
    let max_parallel = levels.iter().map(|l| l.packages.len()).max().unwrap_or(0);
    println!(
        "  Parallel publishing: {}",
        if max_parallel > 1 {
            "enabled"
        } else {
            "sequential"
        }
    );
    println!("  Max concurrent packages: {}", max_parallel);
    println!("  Total publish levels: {}", total_levels);

    // Rough time estimate (assuming ~30s per package + network overhead)
    let total_packages = ws.plan.packages.len();
    let estimated_sequential_secs = total_packages * 30;
    let estimated_parallel_secs = levels.iter().map(|_l| 30).sum::<usize>();
    println!(
        "  Estimated time (sequential): ~{}s ({:.1}min)",
        estimated_sequential_secs,
        estimated_sequential_secs as f64 / 60.0
    );
    println!(
        "  Estimated time (parallel): ~{}s ({:.1}min)",
        estimated_parallel_secs,
        estimated_parallel_secs as f64 / 60.0
    );
    println!();

    // Show final publish order
    println!("=== Full Publish Order ===");
    println!();
    for (idx, p) in ws.plan.packages.iter().enumerate() {
        let level = levels
            .iter()
            .find(|l| l.packages.iter().any(|lp| lp.name == p.name));
        let level_str = level
            .map(|l| format!("[Level {}]", l.level))
            .unwrap_or_else(|| "[?]".to_string());
        println!("  {:>3}. {} {} @{}", idx + 1, level_str, p.name, p.version);
    }
}

fn print_preflight(rep: &PreflightReport, format: &str) {
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(rep).expect("serialize preflight report");
            println!("{}", json);
        }
        _ => {
            println!("Preflight Report");
            println!("===============");
            println!();
            println!("Plan ID: {}", rep.plan_id);
            println!("Timestamp: {}", rep.timestamp.format("%Y-%m-%dT%H:%M:%SZ"));
            println!();
            println!(
                "Token Detected: {}",
                if rep.token_detected { "✓" } else { "✗" }
            );
            println!();

            // Display finishability with color-coded status
            let (finishability_color, finishability_text) = match rep.finishability {
                Finishability::Proven => ("\x1b[32m", "PROVEN"),
                Finishability::NotProven => ("\x1b[33m", "NOT PROVEN"),
                Finishability::Failed => ("\x1b[31m", "FAILED"),
            };
            println!(
                "Finishability: {}{}",
                finishability_color, finishability_text
            );
            println!();

            // Display packages in table format
            println!("Packages:");
            println!(
                "┌─────────────────────┬─────────┬──────────┬──────────┬───────────────┬─────────────┬─────────────┐"
            );
            println!(
                "│ Package             │ Version │ Published│ New Crate │ Auth Type     │ Ownership   │ Dry-run     │"
            );
            println!(
                "├─────────────────────┼─────────┼──────────┼──────────┼───────────────┼─────────────┼─────────────┤"
            );
            for p in &rep.packages {
                let published = if p.already_published { "Yes" } else { "No" };
                let new_crate = if p.is_new_crate { "Yes" } else { "No" };
                let auth_type = match p.auth_type {
                    Some(shipper_core::types::AuthType::Token) => "Token",
                    Some(shipper_core::types::AuthType::TrustedPublishing) => "Trusted",
                    Some(shipper_core::types::AuthType::Unknown) => "Unknown",
                    None => "-",
                };
                let ownership = if p.ownership_verified { "✓" } else { "✗" };
                let dry_run = if p.dry_run_passed { "✓" } else { "✗" };

                println!(
                    "│ {:<19} │ {:<7} │ {:<8} │ {:<8} │ {:<13} │ {:<11} │ {:<11} │",
                    p.name, p.version, published, new_crate, auth_type, ownership, dry_run
                );
            }
            println!(
                "└─────────────────────┴─────────┴──────────┴──────────┴───────────────┴─────────────┴─────────────┘"
            );
            println!();

            // Display dry-run failures if any
            let failed_packages: Vec<_> = rep
                .packages
                .iter()
                .filter(|p| !p.dry_run_passed && p.dry_run_output.is_some())
                .collect();

            if !failed_packages.is_empty() {
                println!("Dry-run Failures:");
                println!("-----------------");
                for p in failed_packages {
                    println!("Package: {}@{}", p.name, p.version);
                    println!("{}", p.dry_run_output.as_ref().unwrap());
                    println!();
                }
            } else if rep.finishability == Finishability::Failed && rep.dry_run_output.is_some() {
                // Check if workspace dry-run failed
                println!("Workspace Dry-run Failure:");
                println!("--------------------------");
                println!("{}", rep.dry_run_output.as_ref().unwrap());
                println!();
            }

            // Summary
            let total = rep.packages.len();
            let already_published = rep.packages.iter().filter(|p| p.already_published).count();
            let new_crates = rep.packages.iter().filter(|p| p.is_new_crate).count();
            let ownership_verified = rep.packages.iter().filter(|p| p.ownership_verified).count();
            let dry_run_passed = rep.packages.iter().filter(|p| p.dry_run_passed).count();

            println!("Summary:");
            println!("  Total packages: {}", total);
            println!("  Already published: {}", already_published);
            println!("  New crates: {}", new_crates);
            println!("  Ownership verified: {}", ownership_verified);
            println!("  Dry-run passed: {}", dry_run_passed);
            println!();

            // What to do next guidance
            println!("What to do next:");
            println!("-----------------");
            match rep.finishability {
                Finishability::Proven => {
                    println!(
                        "\x1b[32m✓ All checks passed. Ready to publish with: shipper publish\x1b[0m"
                    );
                }
                Finishability::NotProven => {
                    println!(
                        "\x1b[33m⚠ Some checks could not be verified. You can still publish, but may encounter permission issues. Use `shipper publish --policy fast` to proceed.\x1b[0m"
                    );
                }
                Finishability::Failed => {
                    println!(
                        "\x1b[31m✗ Preflight failed. Please fix the issues above before publishing.\x1b[0m"
                    );
                }
            }
        }
    }
}

fn print_receipt(
    receipt: &shipper_core::types::Receipt,
    workspace_root: &Path,
    state_dir: &Path,
    format: &str,
) {
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(receipt).expect("serialize receipt");
            println!("{}", json);
        }
        _ => {
            println!("plan_id: {}", receipt.plan_id);
            println!(
                "registry: {} ({})",
                receipt.registry.name, receipt.registry.api_base
            );

            let abs_state = if state_dir.is_absolute() {
                state_dir.to_path_buf()
            } else {
                workspace_root.join(state_dir)
            };

            println!(
                "state:   {}/{}",
                abs_state.display(),
                shipper_core::state::execution_state::STATE_FILE
            );
            println!(
                "receipt: {}/{}",
                abs_state.display(),
                shipper_core::state::execution_state::RECEIPT_FILE
            );
            println!(
                "events:   {}/{}",
                abs_state.display(),
                shipper_core::state::events::EVENTS_FILE
            );
            println!();

            for p in &receipt.packages {
                println!(
                    "{}@{}: {:?} (attempts={}, {}ms)",
                    p.name, p.version, p.state, p.attempts, p.duration_ms
                );
                // Show evidence summary
                if !p.evidence.attempts.is_empty() {
                    println!("  Evidence:");
                    for attempt in &p.evidence.attempts {
                        println!(
                            "    Attempt {}: exit={}, duration={}ms",
                            attempt.attempt_number,
                            attempt.exit_code,
                            attempt.duration.as_millis()
                        );
                        if !attempt.stdout_tail.is_empty() {
                            println!(
                                "      stdout (last {} lines):",
                                attempt.stdout_tail.lines().count()
                            );
                            for line in attempt.stdout_tail.lines().take(5) {
                                println!("        {}", line);
                            }
                        }
                        if !attempt.stderr_tail.is_empty() {
                            println!(
                                "      stderr (last {} lines):",
                                attempt.stderr_tail.lines().count()
                            );
                            for line in attempt.stderr_tail.lines().take(5) {
                                println!("        {}", line);
                            }
                        }
                    }
                }
                if !p.evidence.readiness_checks.is_empty() {
                    println!(
                        "  Readiness checks: {} attempts",
                        p.evidence.readiness_checks.len()
                    );
                    for check in &p.evidence.readiness_checks {
                        println!(
                            "    Poll {}: visible={}, delay_before={}ms",
                            check.attempt,
                            check.visible,
                            check.delay_before.as_millis()
                        );
                    }
                }
            }
        }
    }
}

fn run_inspect_events(ws: &plan::PlannedWorkspace, opts: &RuntimeOptions) -> Result<()> {
    let state_dir = if opts.state_dir.is_absolute() {
        opts.state_dir.clone()
    } else {
        ws.workspace_root.join(&opts.state_dir)
    };

    let events_path = shipper_core::state::events::events_path(&state_dir);
    let event_log = shipper_core::state::events::EventLog::read_from_file(&events_path)
        .with_context(|| format!("failed to read event log from {}", events_path.display()))?;

    println!("Event log: {}", events_path.display());
    println!();

    for event in event_log.all_events() {
        let json = serde_json::to_string(event).expect("serialize event");
        println!("{}", json);
    }

    Ok(())
}

fn run_inspect_receipt(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    format: &str,
) -> Result<()> {
    let state_dir = if opts.state_dir.is_absolute() {
        opts.state_dir.clone()
    } else {
        ws.workspace_root.join(&opts.state_dir)
    };

    let receipt_path = shipper_core::state::execution_state::receipt_path(&state_dir);
    let content = std::fs::read_to_string(&receipt_path)
        .with_context(|| format!("failed to read receipt from {}", receipt_path.display()))?;

    let receipt: shipper_core::types::Receipt = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse receipt from {}", receipt_path.display()))?;

    if format == "json" {
        let json = serde_json::to_string_pretty(&receipt).expect("serialize receipt");
        println!("{}", json);
        return Ok(());
    }

    // Display receipt in human-readable format
    println!("Receipt");
    println!("=======");
    println!();
    println!("Plan ID: {}", receipt.plan_id);
    println!(
        "Registry: {} ({})",
        receipt.registry.name, receipt.registry.api_base
    );
    println!(
        "Started: {}",
        receipt.started_at.format("%Y-%m-%dT%H:%M:%SZ")
    );
    println!(
        "Finished: {}",
        receipt.finished_at.format("%Y-%m-%dT%H:%M:%SZ")
    );
    println!(
        "Duration: {}ms",
        (receipt.finished_at - receipt.started_at).num_milliseconds()
    );
    println!();

    // Display Git context if available
    if let Some(git) = &receipt.git_context {
        println!("Git Context:");
        println!("------------");
        if let Some(commit) = &git.commit {
            println!("  Commit: {}", commit);
        }
        if let Some(branch) = &git.branch {
            println!("  Branch: {}", branch);
        }
        if let Some(tag) = &git.tag {
            println!("  Tag: {}", tag);
        }
        if let Some(dirty) = git.dirty {
            println!("  Dirty: {}", if dirty { "Yes" } else { "No" });
        }
        println!();
    }

    // Display environment fingerprint
    println!("Environment:");
    println!("------------");
    println!("  Shipper: {}", receipt.environment.shipper_version);
    if let Some(cargo) = &receipt.environment.cargo_version {
        println!("  Cargo: {}", cargo);
    }
    if let Some(rust) = &receipt.environment.rust_version {
        println!("  Rust: {}", rust);
    }
    println!("  OS: {}", receipt.environment.os);
    println!("  Arch: {}", receipt.environment.arch);
    println!();

    // Display packages
    println!("Packages:");
    println!("---------");
    for p in &receipt.packages {
        let state_str = match &p.state {
            shipper_core::types::PackageState::Published => "\x1b[32mPublished\x1b[0m",
            shipper_core::types::PackageState::Pending => "Pending",
            shipper_core::types::PackageState::Uploaded => "\x1b[33mUploaded\x1b[0m",
            shipper_core::types::PackageState::Skipped { reason } => {
                &format!("Skipped: {}", reason)
            }
            shipper_core::types::PackageState::Failed { class, message } => {
                &format!("\x1b[31mFailed ({:?}): {}\x1b[0m", class, message)
            }
            shipper_core::types::PackageState::Ambiguous { message } => {
                &format!("\x1b[33mAmbiguous: {}\x1b[0m", message)
            }
        };
        println!(
            "  {}@{}: {} (attempts={}, {}ms)",
            p.name, p.version, state_str, p.attempts, p.duration_ms
        );
    }

    Ok(())
}

fn run_status(ws: &plan::PlannedWorkspace, reporter: &mut dyn Reporter) -> Result<()> {
    reporter.info("initializing registry client...");
    let reg = shipper_core::registry::RegistryClient::new(ws.plan.registry.clone())?;

    println!("plan_id: {}", ws.plan.plan_id);
    println!();

    for p in &ws.plan.packages {
        let exists = reg.version_exists(&p.name, &p.version)?;
        let status = if exists { "published" } else { "missing" };
        println!("{}@{}: {status}", p.name, p.version);
    }

    Ok(())
}

fn run_doctor(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
    reporter: &mut dyn Reporter,
) -> Result<()> {
    println!("Shipper Doctor - Diagnostics Report");
    println!("----------------------------------");
    println!("workspace_root: {}", ws.workspace_root.display());
    println!(
        "registry: {} ({})",
        ws.plan.registry.name, ws.plan.registry.api_base
    );

    // 1. Check Authentication
    let auth_type = shipper_core::auth::detect_auth_type(&ws.plan.registry.name)?;
    let auth_label = match auth_type {
        Some(shipper_core::types::AuthType::Token) => "token (detected)",
        Some(shipper_core::types::AuthType::TrustedPublishing) => "trusted (detected)",
        Some(shipper_core::types::AuthType::Unknown) => "unknown",
        None => "NONE FOUND (set CARGO_REGISTRY_TOKEN)",
    };
    println!("auth_type: {}", auth_label);

    // 2. Check State Directory
    let abs_state = if opts.state_dir.is_absolute() {
        opts.state_dir.clone()
    } else {
        ws.workspace_root.join(&opts.state_dir)
    };
    println!("state_dir: {}", abs_state.display());

    if abs_state.exists() {
        if let Ok(meta) = std::fs::metadata(&abs_state) {
            println!("state_dir_writable: {}", !meta.permissions().readonly());
        }
    } else {
        println!("state_dir_exists: false (will be created)");
    }

    // 3. Check Tools
    println!();
    print_cmd_version("cargo", reporter);
    print_cmd_version("git", reporter);

    // 4. Network Connectivity (Best Effort)
    println!();
    reporter.info("checking registry connectivity...");
    let reg_client = shipper_core::registry::RegistryClient::new(ws.plan.registry.clone())?;

    match reg_client.crate_exists("serde") {
        Ok(_) => println!("registry_reachable: true"),
        Err(e) => reporter.warn(&format!("registry_reachable: false ({e:#})")),
    }

    let index_base = ws.plan.registry.get_index_base();
    println!("index_base: {}", index_base);

    // 5. Check Git State
    println!();
    match shipper_core::git::collect_git_context() {
        Some(git) => {
            println!("git_commit: {}", git.commit.unwrap_or_else(|| "-".into()));
            println!("git_branch: {}", git.branch.unwrap_or_else(|| "-".into()));
            println!("git_dirty: {}", git.dirty.unwrap_or(false));
        }
        None => println!("git_context: not a git repository"),
    }

    // 6. Encryption Check
    if opts.encryption.enabled {
        println!();
        println!("encryption: enabled");
        if opts.encryption.passphrase.is_some() {
            println!("encryption_key_source: config");
        } else if let Some(ref env_var) = opts.encryption.env_var {
            let present = std::env::var(env_var).is_ok();
            println!("encryption_key_source: env ({})", env_var);
            println!("encryption_key_present: {}", present);
        }
    }

    println!();
    println!("Diagnostics complete.");

    Ok(())
}

fn print_cmd_version(cmd: &str, reporter: &mut dyn Reporter) {
    let out = Command::new(cmd).arg("--version").output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            println!("{cmd}: {s}");
        }
        Ok(o) => {
            reporter.warn(&format!(
                "{cmd} --version failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(e) => {
            reporter.warn(&format!("unable to run {cmd} --version: {e}"));
        }
    }
}

fn run_ci(ci_cmd: CiCommands, state_dir: &Path, workspace_root: &Path) -> Result<()> {
    let abs_state = if state_dir.is_absolute() {
        state_dir.to_path_buf()
    } else {
        workspace_root.join(state_dir)
    };

    match ci_cmd {
        CiCommands::GitHubActions => {
            println!("# GitHub Actions workflow snippet for Shipper");
            println!("# Add these steps to your workflow file");
            println!();
            println!("# Restore Shipper State (cache for faster restores)");
            println!("- name: Restore Shipper State");
            println!("  uses: actions/cache@v3");
            println!("  with:");
            println!("    path: {}/", abs_state.display());
            println!("    key: shipper-${{{{ github.sha }}}}");
            println!("    restore-keys: |");
            println!("      shipper-");
            println!();
            println!("# Restore Shipper State (artifact for resumability)");
            println!("- name: Restore Shipper State Artifact");
            println!("  uses: actions/download-artifact@v4");
            println!("  with:");
            println!("    name: shipper-state");
            println!("    path: {}/", abs_state.display());
            println!("  continue-on-error: true");
            println!();
            println!("# Run shipper publish (will resume if state exists)");
            println!("- name: Publish Crates");
            println!("  run: shipper publish --quiet");
            println!("  env:");
            println!("    CARGO_REGISTRY_TOKEN: ${{{{ secrets.CARGO_REGISTRY_TOKEN }}}}");
            println!();
            println!("# Save Shipper State (even if publish fails)");
            println!("- name: Save Shipper State");
            println!("  if: always()");
            println!("  uses: actions/upload-artifact@v3");
            println!("  with:");
            println!("    name: shipper-state");
            println!("    path: {}/", abs_state.display());
        }
        CiCommands::GitLab => {
            println!("# GitLab CI snippet for Shipper");
            println!("# Add this to your .gitlab-ci.yml");
            println!();
            println!("publish:");
            println!("  image: rust:latest");
            println!("  stage: publish");
            println!("  cache:");
            println!("    key: ${{CI_COMMIT_REF_SLUG}}");
            println!("    paths:");
            println!("      - {}/", abs_state.display());
            println!("      - target/");
            println!("  script:");
            println!("    - cargo install shipper-cli --locked");
            println!("    - shipper publish --quiet");
            println!("  variables:");
            println!("    CARGO_TERM_COLOR: \"always\"");
            println!("    # Configure this in GitLab CI/CD settings (masked, protected)");
            println!("    # CARGO_REGISTRY_TOKEN: \"...\"");
            println!("  artifacts:");
            println!("    paths:");
            println!("      - {}/", abs_state.display());
            println!("    expire_in: 1 day");
            println!("    when: always");
        }
        CiCommands::CircleCI => {
            println!("# CircleCI config snippet for Shipper");
            println!("# Add this to your .circleci/config.yml");
            println!();
            println!("version: 2.1");
            println!();
            println!("jobs:");
            println!("  publish:");
            println!("    docker:");
            println!("      - image: cimg/rust:latest");
            println!("    steps:");
            println!("      - checkout");
            println!("      - restore_cache:");
            println!("          keys:");
            println!("            - shipper-state-{{{{ .Branch }}}}-{{{{ .Revision }}}}");
            println!("            - shipper-state-{{{{ .Branch }}}}");
            println!("            - shipper-state-");
            println!("      - run:");
            println!("          name: Install Shipper");
            println!("          command: cargo install shipper-cli --locked");
            println!("      - run:");
            println!("          name: Publish Crates");
            println!("          command: shipper publish --quiet");
            println!("          environment:");
            println!("            CARGO_REGISTRY_TOKEN: ${{{{ CARGO_REGISTRY_TOKEN }}}}");
            println!("      - save_cache:");
            println!("          key: shipper-state-{{{{ .Branch }}}}-{{{{ .Revision }}}}");
            println!("          paths:");
            println!("            - {}", abs_state.display());
            println!("      - store_artifacts:");
            println!("          path: {}", abs_state.display());
            println!("          destination: shipper-state");
            println!();
            println!("workflows:");
            println!("  version: 2");
            println!("  publish:");
            println!("    jobs:");
            println!("      - publish:");
            println!("          filters:");
            println!("            branches:");
            println!("              only: main");
            println!("          context: cargo-registry");
        }
        CiCommands::AzureDevOps => {
            println!("# Azure DevOps pipeline snippet for Shipper");
            println!("# Add this to your azure-pipelines.yml");
            println!();
            println!("trigger:");
            println!("  - main");
            println!();
            println!("pool:");
            println!("  vmImage: 'ubuntu-latest'");
            println!();
            println!("variables:");
            println!("  CARGO_HOME: $(Pipeline.Workspace)/.cargo");
            println!();
            println!("steps:");
            println!("  - task: Cache@2");
            println!("    displayName: 'Cache Cargo and Shipper State'");
            println!("    inputs:");
            println!("      key: 'shipper | \"$(Agent.OS)\" | \"$(Build.SourceVersion)\"'");
            println!("      restoreKeys: |");
            println!("        shipper | \"$(Agent.OS)\"");
            println!("        shipper");
            println!("      path: $(CARGO_HOME)");
            println!("      cacheHitVar: CACHE_RESTORED");
            println!();
            println!("  - script: cargo install shipper-cli --locked");
            println!("    displayName: 'Install Shipper'");
            println!();
            println!("  - script: shipper publish --quiet");
            println!("    displayName: 'Publish Crates'");
            println!("    env:");
            println!("      CARGO_REGISTRY_TOKEN: $(CARGO_REGISTRY_TOKEN)");
            println!();
            println!("  - publish: {}", abs_state.display());
            println!("    displayName: 'Publish Shipper State Artifact'");
            println!("    condition: succeededOrFailed()");
            println!("    artifact: 'shipper-state'");
        }
    }

    Ok(())
}

fn run_clean(
    state_dir: &PathBuf,
    workspace_root: &Path,
    keep_receipt: bool,
    force: bool,
) -> Result<()> {
    let abs_state = if state_dir.is_absolute() {
        state_dir.clone()
    } else {
        workspace_root.join(state_dir)
    };

    if !abs_state.exists() {
        println!("State directory does not exist: {}", abs_state.display());
        return Ok(());
    }

    // Identify all directories to clean (base + any registry subdirs)
    let mut dirs_to_clean = vec![abs_state.clone()];
    if let Ok(entries) = std::fs::read_dir(&abs_state) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type()
                && file_type.is_dir()
                && entry.file_name() != "cache"
            {
                dirs_to_clean.push(entry.path());
            }
        }
    }

    for dir in dirs_to_clean {
        clean_single_dir(&dir, workspace_root, keep_receipt, force)?;
    }

    println!("Clean complete");
    Ok(())
}

fn clean_single_dir(
    dir: &Path,
    workspace_root: &Path,
    keep_receipt: bool,
    force: bool,
) -> Result<()> {
    let state_path = dir.join(shipper_core::state::execution_state::STATE_FILE);
    let receipt_path = dir.join(shipper_core::state::execution_state::RECEIPT_FILE);
    let events_path = dir.join(shipper_core::state::events::EVENTS_FILE);
    let lock_path = shipper_core::lock::lock_path(dir, Some(workspace_root));

    // Check for active lock
    if lock_path.exists() {
        if force {
            eprintln!(
                "[warn] --force specified; removing lock file: {}",
                lock_path.display()
            );
            std::fs::remove_file(&lock_path)
                .with_context(|| format!("failed to remove lock file {}", lock_path.display()))?;
        } else {
            match shipper_core::lock::LockFile::read_lock_info(dir, Some(workspace_root)) {
                Ok(lock_info) => {
                    eprintln!("[warn] Active lock found in {}:", dir.display());
                    eprintln!("[warn]   PID: {}", lock_info.pid);
                    eprintln!("[warn]   Hostname: {}", lock_info.hostname);
                    eprintln!("[warn]   Acquired at: {}", lock_info.acquired_at);
                    eprintln!("[warn]   Plan ID: {:?}", lock_info.plan_id);
                }
                Err(err) => {
                    eprintln!(
                        "[warn] Active lock found in {} but metadata could not be read: {err:#}",
                        dir.display()
                    );
                }
            }
            eprintln!("[warn] Use --force to override the lock");
            bail!("cannot clean: active lock exists in {}", dir.display());
        }
    }

    // Remove state file
    if state_path.exists() {
        std::fs::remove_file(&state_path)
            .with_context(|| format!("failed to remove state file {}", state_path.display()))?;
        println!("Removed: {}", state_path.display());
    }

    // Remove events file
    if events_path.exists() {
        std::fs::remove_file(&events_path)
            .with_context(|| format!("failed to remove events file {}", events_path.display()))?;
        println!("Removed: {}", events_path.display());
    }

    // Optionally remove receipt file
    if !keep_receipt && receipt_path.exists() {
        std::fs::remove_file(&receipt_path)
            .with_context(|| format!("failed to remove receipt file {}", receipt_path.display()))?;
        println!("Removed: {}", receipt_path.display());
    } else if keep_receipt && receipt_path.exists() {
        println!(
            "Kept: {} (--keep-receipt specified)",
            receipt_path.display()
        );
    }

    // Remove cache directory if exists
    let cache_dir = dir.join("cache");
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir)
            .with_context(|| format!("failed to remove cache directory {}", cache_dir.display()))?;
        println!("Removed: {}", cache_dir.display());
    }

    Ok(())
}

fn run_config(cmd: ConfigCommands) -> Result<()> {
    match cmd {
        ConfigCommands::Init { output } => {
            let template = ShipperConfig::default_toml_template();
            std::fs::write(&output, template)
                .with_context(|| format!("Failed to write config file to {}", output.display()))?;
            println!("Created configuration file: {}", output.display());
            println!();
            println!("Edit the file to customize shipper settings for your workspace.");
            println!("Run `shipper config validate` to check the configuration.");
        }
        ConfigCommands::Validate { path } => {
            if !path.exists() {
                bail!("Config file not found: {}", path.display());
            }
            let config = ShipperConfig::load_from_file(&path)
                .with_context(|| format!("Failed to load config file: {}", path.display()))?;
            config.validate().with_context(|| {
                format!("Configuration validation failed for {}", path.display())
            })?;
            println!("Configuration file is valid: {}", path.display());
        }
    }
    Ok(())
}

fn run_completion(shell: &Shell) -> Result<()> {
    clap_complete::generate(
        *shell,
        &mut Cli::command(),
        "shipper",
        &mut std::io::stdout(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

    #[derive(Default)]
    struct TestReporter {
        infos: Vec<String>,
        warns: Vec<String>,
        errors: Vec<String>,
    }

    impl Reporter for TestReporter {
        fn info(&mut self, msg: &str) {
            self.infos.push(msg.to_string());
        }

        fn warn(&mut self, msg: &str) {
            self.warns.push(msg.to_string());
        }

        fn error(&mut self, msg: &str) {
            self.errors.push(msg.to_string());
        }
    }

    #[test]
    fn parse_duration_handles_valid_and_invalid_inputs() {
        assert!(parse_duration("1s").is_ok());
        assert!(parse_duration("nope").is_err());
    }

    #[test]
    fn global_flags_parse_after_subcommand() {
        let cli = Cli::try_parse_from([
            "shipper",
            "preflight",
            "--allow-dirty",
            "--strict-ownership",
            "--verify-mode",
            "package",
            "--policy",
            "safe",
            "--format",
            "json",
        ])
        .expect("parse CLI");

        assert!(matches!(cli.cmd, Commands::Preflight));
        assert!(cli.allow_dirty);
        assert!(cli.strict_ownership);
        assert_eq!(cli.verify_mode.as_deref(), Some("package"));
        assert_eq!(cli.policy.as_deref(), Some("safe"));
        assert_eq!(cli.format, "json");
    }

    #[test]
    fn cli_reporter_methods_are_callable() {
        let mut rep = CliReporter { quiet: false };
        rep.info("info");
        rep.warn("warn");
        rep.error("error");
    }

    #[test]
    fn print_cmd_version_reports_missing_command() {
        let mut reporter = TestReporter::default();
        print_cmd_version("definitely-not-a-real-command-shipper", &mut reporter);
        assert!(reporter.warns.iter().any(|w| w.contains("unable to run")));
    }

    #[test]
    #[serial]
    fn print_cmd_version_reports_non_zero_exit() {
        let td = tempdir().expect("tempdir");
        let bin_dir = td.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");

        #[cfg(windows)]
        let cmd_path = {
            let p = bin_dir.join("badver.cmd");
            fs::write(
                &p,
                "@echo off\r\necho bad version error 1>&2\r\nexit /b 1\r\n",
            )
            .expect("write");
            p
        };

        #[cfg(not(windows))]
        let cmd_path = {
            use std::os::unix::fs::PermissionsExt;

            let p = bin_dir.join("badver");
            fs::write(
                &p,
                "#!/usr/bin/env sh\necho bad version error >&2\nexit 1\n",
            )
            .expect("write");
            let mut perms = fs::metadata(&p).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms).expect("chmod");
            p
        };

        let mut reporter = TestReporter::default();
        print_cmd_version(cmd_path.to_str().expect("utf8"), &mut reporter);
        assert!(
            reporter
                .warns
                .iter()
                .any(|w| w.contains("--version failed"))
        );
    }

    #[test]
    fn test_reporter_collects_all_levels() {
        let mut reporter = TestReporter::default();
        reporter.info("i");
        reporter.warn("w");
        reporter.error("e");
        assert_eq!(reporter.infos, vec!["i".to_string()]);
        assert_eq!(reporter.warns, vec!["w".to_string()]);
        assert_eq!(reporter.errors, vec!["e".to_string()]);
    }

    #[test]
    #[serial]
    fn run_doctor_supports_absolute_state_dir() {
        let td = tempdir().expect("tempdir");
        let ws = plan::PlannedWorkspace {
            workspace_root: td.path().to_path_buf(),
            plan: shipper_core::types::ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "plan-x".to_string(),
                created_at: chrono::Utc::now(),
                registry: Registry::crates_io(),
                packages: vec![],
                dependencies: std::collections::BTreeMap::new(),
            },
            skipped: vec![],
        };

        let state_dir = td.path().join("abs-state");
        let opts = RuntimeOptions {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: false,
            no_verify: false,
            max_attempts: 1,
            base_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(0),
            retry_strategy: shipper_core::retry::RetryStrategyType::Exponential,
            retry_jitter: 0.5,
            retry_per_error: shipper_core::retry::PerErrorConfig::default(),
            verify_timeout: Duration::from_millis(0),
            verify_poll_interval: Duration::from_millis(0),
            state_dir: state_dir.clone(),
            force_resume: false,
            force: false,
            lock_timeout: Duration::from_secs(3600),
            policy: shipper_core::types::PublishPolicy::Safe,
            verify_mode: shipper_core::types::VerifyMode::Workspace,
            readiness: shipper_core::types::ReadinessConfig::default(),
            output_lines: 50,
            parallel: shipper_core::types::ParallelConfig::default(),
            webhook: shipper_core::webhook::WebhookConfig::default(),
            encryption: shipper_core::encryption::EncryptionConfig::default(),
            registries: vec![],
            resume_from: None,
            rehearsal_registry: None,
            rehearsal_skip: false,
            rehearsal_smoke_install: None,
        };

        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<String>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
                (
                    "CARGO_HOME",
                    Some(
                        td.path()
                            .join("cargo-home")
                            .to_str()
                            .expect("utf8")
                            .to_string(),
                    ),
                ),
            ],
            || {
                let mut reporter = TestReporter::default();
                run_doctor(&ws, &opts, &mut reporter).expect("doctor");
            },
        );
    }

    #[test]
    #[serial]
    fn run_doctor_restores_env_when_old_values_are_missing_or_present() {
        let td = tempdir().expect("tempdir");
        let ws = plan::PlannedWorkspace {
            workspace_root: td.path().to_path_buf(),
            plan: shipper_core::types::ReleasePlan {
                plan_version: "1".to_string(),
                plan_id: "plan-y".to_string(),
                created_at: chrono::Utc::now(),
                registry: Registry::crates_io(),
                packages: vec![],
                dependencies: std::collections::BTreeMap::new(),
            },
            skipped: vec![],
        };

        let opts = RuntimeOptions {
            allow_dirty: true,
            skip_ownership_check: true,
            strict_ownership: false,
            no_verify: false,
            max_attempts: 1,
            base_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(0),
            retry_strategy: shipper_core::retry::RetryStrategyType::Exponential,
            retry_jitter: 0.5,
            retry_per_error: shipper_core::retry::PerErrorConfig::default(),
            verify_timeout: Duration::from_millis(0),
            verify_poll_interval: Duration::from_millis(0),
            state_dir: td.path().join("abs-state-2"),
            force_resume: false,
            force: false,
            lock_timeout: Duration::from_secs(3600),
            policy: shipper_core::types::PublishPolicy::Safe,
            verify_mode: shipper_core::types::VerifyMode::Workspace,
            readiness: shipper_core::types::ReadinessConfig::default(),
            output_lines: 50,
            parallel: shipper_core::types::ParallelConfig::default(),
            webhook: shipper_core::webhook::WebhookConfig::default(),
            encryption: shipper_core::encryption::EncryptionConfig::default(),
            registries: vec![],
            resume_from: None,
            rehearsal_registry: None,
            rehearsal_skip: false,
            rehearsal_smoke_install: None,
        };

        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        temp_env::with_vars(
            [
                ("CARGO_REGISTRY_TOKEN", None::<String>),
                ("CARGO_REGISTRIES_CRATES_IO_TOKEN", None::<String>),
                (
                    "CARGO_HOME",
                    Some(
                        td.path()
                            .join("cargo-home")
                            .to_str()
                            .expect("utf8")
                            .to_string(),
                    ),
                ),
            ],
            || {
                let mut reporter = TestReporter::default();
                run_doctor(&ws, &opts, &mut reporter).expect("doctor");
            },
        );
    }

    #[test]
    fn config_init_creates_file() {
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join("test-config.toml");

        run_config(ConfigCommands::Init {
            output: config_path.clone(),
        })
        .expect("config init should succeed");

        assert!(config_path.exists(), "config file should be created");

        let content = fs::read_to_string(&config_path).expect("read config file");
        assert!(
            content.contains("[policy]"),
            "config should contain [policy] section"
        );
        assert!(
            content.contains("[readiness]"),
            "config should contain [readiness] section"
        );
    }

    #[test]
    fn config_validate_valid_file() {
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join("test-config.toml");

        // Create a valid config
        let valid_config = r#"
[policy]
mode = "safe"

[verify]
mode = "workspace"

[readiness]
enabled = true
method = "api"
initial_delay = "1s"
max_delay = "60s"
max_total_wait = "5m"
poll_interval = "2s"
jitter_factor = 0.5

[output]
lines = 50

[retry]
max_attempts = 6
base_delay = "2s"
max_delay = "2m"

[lock]
timeout = "1h"
"#;

        fs::write(&config_path, valid_config).expect("write config file");

        run_config(ConfigCommands::Validate {
            path: config_path.clone(),
        })
        .expect("config validate should succeed for valid file");
    }

    #[test]
    fn config_validate_invalid_file() {
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join("test-config.toml");

        // Create an invalid config (output_lines = 0)
        let invalid_config = r#"
[output]
lines = 0
"#;

        fs::write(&config_path, invalid_config).expect("write config file");

        let result = run_config(ConfigCommands::Validate {
            path: config_path.clone(),
        });

        assert!(
            result.is_err(),
            "config validate should fail for invalid file"
        );
        let err = result.unwrap_err().to_string();
        // The error is wrapped in context, so check the full message
        assert!(
            err.contains("output.lines must be greater than 0")
                || err.contains("Configuration validation failed"),
            "error should mention output.lines or validation failed"
        );
    }

    #[test]
    fn config_validate_missing_file() {
        let td = tempdir().expect("tempdir");
        let config_path = td.path().join("nonexistent-config.toml");

        let result = run_config(ConfigCommands::Validate {
            path: config_path.clone(),
        });

        assert!(
            result.is_err(),
            "config validate should fail for missing file"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found") || err.contains("Config file not found"),
            "error should mention file not found"
        );
    }

    #[test]
    fn config_load_from_workspace() {
        let td = tempdir().expect("tempdir");
        let workspace_root = td.path();

        // No config file exists
        let result = ShipperConfig::load_from_workspace(workspace_root);
        assert!(
            result.is_ok(),
            "load should succeed even without config file"
        );
        assert!(
            result.unwrap().is_none(),
            "should return None when no config exists"
        );

        // Create a config file
        let config_path = workspace_root.join(".shipper.toml");
        let valid_config = r#"
[policy]
mode = "fast"
"#;

        fs::write(&config_path, valid_config).expect("write config file");

        let result = ShipperConfig::load_from_workspace(workspace_root);
        assert!(result.is_ok(), "load should succeed");
        let config = result.unwrap();
        assert!(config.is_some(), "should return Some when config exists");
        assert_eq!(
            config.unwrap().policy.mode,
            shipper_core::config::PublishPolicy::Fast
        );
    }

    #[test]
    fn config_merge_with_cli_overrides() {
        let config = ShipperConfig {
            schema_version: "shipper.config.v1".to_string(),
            policy: shipper_core::config::PolicyConfig {
                mode: shipper_core::config::PublishPolicy::Safe,
            },
            verify: shipper_core::config::VerifyConfig {
                mode: shipper_core::config::VerifyMode::Workspace,
            },
            readiness: shipper_core::config::ReadinessConfig::default(),
            output: shipper_core::config::OutputConfig { lines: 100 },
            lock: shipper_core::config::LockConfig {
                timeout: Duration::from_secs(1800),
            },
            flags: shipper_core::config::FlagsConfig {
                allow_dirty: false,
                skip_ownership_check: false,
                strict_ownership: false,
            },
            retry: shipper_core::config::RetryConfig {
                policy: shipper_core::retry::RetryPolicy::Custom,
                max_attempts: 10,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(300),
                strategy: shipper_core::retry::RetryStrategyType::Exponential,
                jitter: 0.5,
                per_error: shipper_core::retry::PerErrorConfig::default(),
            },
            state_dir: None,
            registry: None,
            registries: shipper_core::config::MultiRegistryConfig::default(),
            parallel: shipper_core::config::ParallelConfig::default(),
            webhook: shipper_core::config::WebhookConfig::default(),
            encryption: shipper_core::config::EncryptionConfigInner::default(),
            storage: shipper_core::config::StorageConfigInner::default(),
            rehearsal: shipper_core::config::RehearsalConfig::default(),
        };

        // CLI overrides some values, leaves others as None
        let cli = CliOverrides {
            allow_dirty: true,
            max_attempts: Some(3),
            output_lines: Some(50),
            policy: Some(shipper_core::config::PublishPolicy::Fast),
            verify_mode: Some(shipper_core::config::VerifyMode::None),
            ..Default::default()
        };

        let merged: RuntimeOptions = config.build_runtime_options(cli);

        // CLI values should win where set
        assert!(merged.allow_dirty, "CLI allow_dirty should win");
        assert_eq!(merged.max_attempts, 3, "CLI max_attempts should win");
        assert_eq!(merged.output_lines, 50, "CLI output_lines should win");
        assert_eq!(
            merged.policy,
            shipper_core::types::PublishPolicy::Fast,
            "CLI policy should win"
        );
        assert_eq!(
            merged.verify_mode,
            shipper_core::types::VerifyMode::None,
            "CLI verify_mode should win"
        );

        // Config values should apply where CLI is None
        assert_eq!(
            merged.base_delay,
            Duration::from_secs(5),
            "config base_delay should apply"
        );
        assert_eq!(
            merged.max_delay,
            Duration::from_secs(300),
            "config max_delay should apply"
        );
        assert_eq!(
            merged.lock_timeout,
            Duration::from_secs(1800),
            "config lock_timeout should apply"
        );
    }

    #[test]
    fn run_clean_errors_when_lock_exists_without_force() {
        let td = tempdir().expect("tempdir");
        let state_dir = PathBuf::from(".shipper");
        let abs_state = td.path().join(&state_dir);
        fs::create_dir_all(&abs_state).expect("mkdir");

        let lock_info = shipper_core::lock::LockInfo {
            pid: 12345,
            hostname: "test-host".to_string(),
            acquired_at: Utc::now(),
            plan_id: Some("plan-123".to_string()),
        };
        let lock_path = shipper_core::lock::lock_path(&abs_state, Some(td.path()));
        fs::write(
            &lock_path,
            serde_json::to_string(&lock_info).expect("serialize"),
        )
        .expect("write lock");

        let err = run_clean(&state_dir, td.path(), false, false).expect_err("must fail");
        assert!(err.to_string().contains("cannot clean: active lock exists"));
        assert!(lock_path.exists());
    }

    #[test]
    fn run_clean_force_removes_lock_and_state_files() {
        let td = tempdir().expect("tempdir");
        let state_dir = PathBuf::from(".shipper");
        let abs_state = td.path().join(&state_dir);
        fs::create_dir_all(&abs_state).expect("mkdir");

        let state_path = abs_state.join(shipper_core::state::execution_state::STATE_FILE);
        let receipt_path = abs_state.join(shipper_core::state::execution_state::RECEIPT_FILE);
        let events_path = abs_state.join(shipper_core::state::events::EVENTS_FILE);
        let lock_path = shipper_core::lock::lock_path(&abs_state, Some(td.path()));

        fs::write(&state_path, "{}").expect("write state");
        fs::write(&receipt_path, "{}").expect("write receipt");
        fs::write(&events_path, "{}").expect("write events");

        let lock_info = shipper_core::lock::LockInfo {
            pid: 12345,
            hostname: "test-host".to_string(),
            acquired_at: Utc::now(),
            plan_id: Some("plan-123".to_string()),
        };
        fs::write(
            &lock_path,
            serde_json::to_string(&lock_info).expect("serialize"),
        )
        .expect("write lock");

        run_clean(&state_dir, td.path(), false, true).expect("clean with force");

        assert!(!state_path.exists(), "state file should be removed");
        assert!(!receipt_path.exists(), "receipt file should be removed");
        assert!(!events_path.exists(), "events file should be removed");
        assert!(!lock_path.exists(), "lock file should be removed");
    }
}
