use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use shipper::config::{CliOverrides, ShipperConfig};
use shipper::engine::{self, Reporter};
use shipper::plan;
use shipper::types::{Finishability, PreflightReport, Registry, ReleaseSpec, RuntimeOptions};

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

    /// Registry API base URL (default: https://crates.io)
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

    /// Output format: text (default) or json
    #[arg(long, default_value = "text", global = true)]
    format: String,

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
    /// Configuration file management.
    #[command(subcommand)]
    Config(ConfigCommands),
}

#[derive(Subcommand, Debug)]
enum CiCommands {
    /// Print GitHub Actions workflow snippet.
    #[command(name = "github-actions")]
    GitHubActions,
    /// Print GitLab CI workflow snippet.
    #[command(name = "gitlab")]
    GitLab,
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

struct CliReporter;

impl Reporter for CliReporter {
    fn info(&mut self, msg: &str) {
        eprintln!("[info] {msg}");
    }

    fn warn(&mut self, msg: &str) {
        eprintln!("[warn] {msg}");
    }

    fn error(&mut self, msg: &str) {
        eprintln!("[error] {msg}");
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle Config commands early (they don't need workspace plan)
    if let Commands::Config(config_cmd) = &cli.cmd {
        return run_config(config_cmd.clone());
    }

    let spec = ReleaseSpec {
        manifest_path: cli.manifest_path.clone(),
        registry: Registry {
            name: cli
                .registry
                .clone()
                .unwrap_or_else(|| "crates-io".to_string()),
            api_base: cli
                .api_base
                .clone()
                .unwrap_or_else(|| "https://crates.io".to_string()),
            index_base: None,
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

    // Apply registry from config if CLI didn't set it
    if let Some(ref cfg) = config
        && let Some(ref reg_config) = cfg.registry
    {
        if cli.registry.is_none() {
            planned.plan.registry.name = reg_config.name.clone();
        }
        if cli.api_base.is_none() {
            planned.plan.registry.api_base = reg_config.api_base.clone();
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
    };

    // Merge CLI overrides with config (or defaults if no config)
    let config_for_merge = config.clone().unwrap_or_default();
    let opts = config_for_merge.build_runtime_options(cli_overrides);

    let mut reporter = CliReporter;

    match cli.cmd {
        Commands::Plan => {
            print_plan(&planned);
        }
        Commands::Preflight => {
            let rep = engine::run_preflight(&planned, &opts, &mut reporter)?;
            print_preflight(&rep, &cli.format);
        }
        Commands::Publish => {
            let receipt = engine::run_publish(&planned, &opts, &mut reporter)?;
            print_receipt(
                &receipt,
                &planned.workspace_root,
                &opts.state_dir,
                &cli.format,
            );
        }
        Commands::Resume => {
            let receipt = engine::run_resume(&planned, &opts, &mut reporter)?;
            print_receipt(
                &receipt,
                &planned.workspace_root,
                &opts.state_dir,
                &cli.format,
            );
        }
        Commands::Status => {
            run_status(&planned, &mut reporter)?;
        }
        Commands::Doctor => {
            run_doctor(&planned, &opts, &mut reporter)?;
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
        Commands::Clean { keep_receipt } => {
            run_clean(&opts.state_dir, &planned.workspace_root, keep_receipt)?;
        }
        Commands::Config(_) => {
            // This should never be reached since we handle Config commands early
            unreachable!("Config commands should be handled before this match");
        }
    }

    Ok(())
}

fn parse_duration(s: &str) -> Result<Duration> {
    humantime::parse_duration(s).with_context(|| format!("invalid duration: {s}"))
}

fn parse_policy(s: &str) -> Result<shipper::types::PublishPolicy> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(shipper::types::PublishPolicy::Safe),
        "balanced" => Ok(shipper::types::PublishPolicy::Balanced),
        "fast" => Ok(shipper::types::PublishPolicy::Fast),
        _ => bail!("invalid policy: {s} (expected: safe, balanced, fast)"),
    }
}

fn parse_verify_mode(s: &str) -> Result<shipper::types::VerifyMode> {
    match s.to_lowercase().as_str() {
        "workspace" => Ok(shipper::types::VerifyMode::Workspace),
        "package" => Ok(shipper::types::VerifyMode::Package),
        "none" => Ok(shipper::types::VerifyMode::None),
        _ => bail!("invalid verify-mode: {s} (expected: workspace, package, none)"),
    }
}

fn parse_readiness_method(s: &str) -> Result<shipper::types::ReadinessMethod> {
    match s.to_lowercase().as_str() {
        "api" => Ok(shipper::types::ReadinessMethod::Api),
        "index" => Ok(shipper::types::ReadinessMethod::Index),
        "both" => Ok(shipper::types::ReadinessMethod::Both),
        _ => bail!("invalid readiness-method: {s} (expected: api, index, both)"),
    }
}

fn print_plan(ws: &plan::PlannedWorkspace) {
    println!("plan_id: {}", ws.plan.plan_id);
    println!(
        "registry: {} ({})",
        ws.plan.registry.name, ws.plan.registry.api_base
    );
    println!("workspace_root: {}", ws.workspace_root.display());
    println!();

    if !ws.skipped.is_empty() {
        println!("Skipped packages:");
        for p in &ws.skipped {
            println!("  - {}@{} ({})", p.name, p.version, p.reason);
        }
        println!();
    }

    for (idx, p) in ws.plan.packages.iter().enumerate() {
        println!("{:>3}. {}@{}", idx + 1, p.name, p.version);
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
                    Some(shipper::types::AuthType::Token) => "Token",
                    Some(shipper::types::AuthType::TrustedPublishing) => "Trusted",
                    Some(shipper::types::AuthType::Unknown) => "Unknown",
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
    receipt: &shipper::types::Receipt,
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
                shipper::state::STATE_FILE
            );
            println!(
                "receipt: {}/{}",
                abs_state.display(),
                shipper::state::RECEIPT_FILE
            );
            println!(
                "events:   {}/{}",
                abs_state.display(),
                shipper::events::EVENTS_FILE
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

    let events_path = shipper::events::events_path(&state_dir);
    let event_log = shipper::events::EventLog::read_from_file(&events_path)
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

    let receipt_path = shipper::state::receipt_path(&state_dir);
    let content = std::fs::read_to_string(&receipt_path)
        .with_context(|| format!("failed to read receipt from {}", receipt_path.display()))?;

    let receipt: shipper::types::Receipt = serde_json::from_str(&content)
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
            shipper::types::PackageState::Published => "\x1b[32mPublished\x1b[0m",
            shipper::types::PackageState::Pending => "Pending",
            shipper::types::PackageState::Uploaded => "\x1b[33mUploaded\x1b[0m",
            shipper::types::PackageState::Skipped { reason } => &format!("Skipped: {}", reason),
            shipper::types::PackageState::Failed { class, message } => {
                &format!("\x1b[31mFailed ({:?}): {}\x1b[0m", class, message)
            }
            shipper::types::PackageState::Ambiguous { message } => {
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
    let reg = shipper::registry::RegistryClient::new(ws.plan.registry.clone())?;

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
    println!("workspace_root: {}", ws.workspace_root.display());
    println!(
        "registry: {} ({})",
        ws.plan.registry.name, ws.plan.registry.api_base
    );

    let auth_type = shipper::auth::detect_auth_type(&ws.plan.registry.name)?;
    println!(
        "token_detected: {}",
        matches!(auth_type, Some(shipper::types::AuthType::Token))
    );
    let auth_label = match auth_type {
        Some(shipper::types::AuthType::Token) => "token",
        Some(shipper::types::AuthType::TrustedPublishing) => "trusted",
        Some(shipper::types::AuthType::Unknown) => "unknown",
        None => "-",
    };
    println!("auth_type: {}", auth_label);

    let abs_state = if opts.state_dir.is_absolute() {
        opts.state_dir.clone()
    } else {
        ws.workspace_root.join(&opts.state_dir)
    };
    println!("state_dir: {}", abs_state.display());

    println!();

    print_cmd_version("cargo", reporter);
    print_cmd_version("git", reporter);

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
            println!("  run: shipper publish");
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
            println!("    - shipper publish");
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
    }

    Ok(())
}

fn run_clean(state_dir: &PathBuf, workspace_root: &Path, keep_receipt: bool) -> Result<()> {
    let abs_state = if state_dir.is_absolute() {
        state_dir.clone()
    } else {
        workspace_root.join(state_dir)
    };

    let state_path = abs_state.join(shipper::state::STATE_FILE);
    let receipt_path = abs_state.join(shipper::state::RECEIPT_FILE);
    let events_path = abs_state.join(shipper::events::EVENTS_FILE);
    let lock_path = abs_state.join(shipper::lock::LOCK_FILE);

    // Check for active lock
    if lock_path.exists() {
        let lock_info = shipper::lock::LockFile::read_lock_info(&abs_state)?;
        eprintln!("[warn] Active lock found:");
        eprintln!("[warn]   PID: {}", lock_info.pid);
        eprintln!("[warn]   Hostname: {}", lock_info.hostname);
        eprintln!("[warn]   Acquired at: {}", lock_info.acquired_at);
        eprintln!("[warn]   Plan ID: {:?}", lock_info.plan_id);
        eprintln!("[warn] Use --force to override the lock");
        bail!("cannot clean: active lock exists");
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

    // Note: We don't remove the state directory itself as it may contain other files
    // and we want to keep the structure for future runs

    println!("Clean complete");
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

#[cfg(test)]
mod tests {
    use std::fs;

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
        let mut rep = CliReporter;
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
            plan: shipper::types::ReleasePlan {
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
            verify_timeout: Duration::from_millis(0),
            verify_poll_interval: Duration::from_millis(0),
            state_dir: state_dir.clone(),
            force_resume: false,
            force: false,
            lock_timeout: Duration::from_secs(3600),
            policy: shipper::types::PublishPolicy::Safe,
            verify_mode: shipper::types::VerifyMode::Workspace,
            readiness: shipper::types::ReadinessConfig::default(),
            output_lines: 50,
            parallel: shipper::types::ParallelConfig::default(),
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
            plan: shipper::types::ReleasePlan {
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
            verify_timeout: Duration::from_millis(0),
            verify_poll_interval: Duration::from_millis(0),
            state_dir: td.path().join("abs-state-2"),
            force_resume: false,
            force: false,
            lock_timeout: Duration::from_secs(3600),
            policy: shipper::types::PublishPolicy::Safe,
            verify_mode: shipper::types::VerifyMode::Workspace,
            readiness: shipper::types::ReadinessConfig::default(),
            output_lines: 50,
            parallel: shipper::types::ParallelConfig::default(),
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
            shipper::types::PublishPolicy::Fast
        );
    }

    #[test]
    fn config_merge_with_cli_overrides() {
        let config = ShipperConfig {
            policy: shipper::config::PolicyConfig {
                mode: shipper::types::PublishPolicy::Safe,
            },
            verify: shipper::config::VerifyConfig {
                mode: shipper::types::VerifyMode::Workspace,
            },
            readiness: shipper::types::ReadinessConfig::default(),
            output: shipper::config::OutputConfig { lines: 100 },
            lock: shipper::config::LockConfig {
                timeout: Duration::from_secs(1800),
            },
            flags: shipper::config::FlagsConfig {
                allow_dirty: false,
                skip_ownership_check: false,
                strict_ownership: false,
            },
            retry: shipper::config::RetryConfig {
                max_attempts: 10,
                base_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(300),
            },
            state_dir: None,
            registry: None,
            parallel: shipper::types::ParallelConfig::default(),
        };

        // CLI overrides some values, leaves others as None
        let cli = CliOverrides {
            allow_dirty: true,
            max_attempts: Some(3),
            output_lines: Some(50),
            policy: Some(shipper::types::PublishPolicy::Fast),
            verify_mode: Some(shipper::types::VerifyMode::None),
            ..Default::default()
        };

        let merged = config.build_runtime_options(cli);

        // CLI values should win where set
        assert!(merged.allow_dirty, "CLI allow_dirty should win");
        assert_eq!(merged.max_attempts, 3, "CLI max_attempts should win");
        assert_eq!(merged.output_lines, 50, "CLI output_lines should win");
        assert_eq!(
            merged.policy,
            shipper::types::PublishPolicy::Fast,
            "CLI policy should win"
        );
        assert_eq!(
            merged.verify_mode,
            shipper::types::VerifyMode::None,
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
}
