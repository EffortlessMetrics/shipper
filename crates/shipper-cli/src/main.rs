use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use shipper::engine::{self, Reporter};
use shipper::plan;
use shipper::types::{Registry, ReleaseSpec, RuntimeOptions};

#[derive(Parser, Debug)]
#[command(name = "shipper", version)]
#[command(about = "Resumable, backoff-aware crates.io publishing for workspaces")]
struct Cli {
    /// Path to the workspace Cargo.toml
    #[arg(long, default_value = "Cargo.toml")]
    manifest_path: PathBuf,

    /// Cargo registry name (default: crates-io)
    #[arg(long, default_value = "crates-io")]
    registry: String,

    /// Registry API base URL (default: https://crates.io)
    #[arg(long, default_value = "https://crates.io")]
    api_base: String,

    /// Restrict to specific packages (repeatable). If omitted, publishes all publishable workspace members.
    #[arg(long = "package")]
    packages: Vec<String>,

    /// Directory for shipper state and receipts (default: .shipper)
    #[arg(long, default_value = ".shipper")]
    state_dir: PathBuf,

    /// Allow publishing from a dirty git working tree.
    #[arg(long)]
    allow_dirty: bool,

    /// Skip owners/permissions preflight.
    #[arg(long)]
    skip_ownership_check: bool,

    /// Fail preflight if ownership checks fail or if no token is available.
    ///
    /// Note: crates.io token scopes may not allow querying owners; this is best-effort.
    #[arg(long)]
    strict_ownership: bool,

    /// Pass --no-verify to cargo publish.
    #[arg(long)]
    no_verify: bool,

    /// Max attempts per crate publish step.
    #[arg(long, default_value_t = 6)]
    max_attempts: u32,

    /// Base backoff delay (e.g. 2s, 500ms)
    #[arg(long, default_value = "2s")]
    base_delay: String,

    /// Max backoff delay (e.g. 2m)
    #[arg(long, default_value = "2m")]
    max_delay: String,

    /// How long to wait for registry visibility after a successful publish.
    #[arg(long, default_value = "2m")]
    verify_timeout: String,

    /// Poll interval for checking registry visibility.
    #[arg(long, default_value = "5s")]
    verify_poll: String,

    /// Force resume even if the computed plan differs from the state file.
    #[arg(long)]
    force_resume: bool,

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

    let spec = ReleaseSpec {
        manifest_path: cli.manifest_path.clone(),
        registry: Registry {
            name: cli.registry.clone(),
            api_base: cli.api_base.clone(),
        },
        selected_packages: if cli.packages.is_empty() {
            None
        } else {
            Some(cli.packages.clone())
        },
    };

    let planned = plan::build_plan(&spec)?;

    let opts = RuntimeOptions {
        allow_dirty: cli.allow_dirty,
        skip_ownership_check: cli.skip_ownership_check,
        strict_ownership: cli.strict_ownership,
        no_verify: cli.no_verify,
        max_attempts: cli.max_attempts,
        base_delay: parse_duration(&cli.base_delay)?,
        max_delay: parse_duration(&cli.max_delay)?,
        verify_timeout: parse_duration(&cli.verify_timeout)?,
        verify_poll_interval: parse_duration(&cli.verify_poll)?,
        state_dir: cli.state_dir.clone(),
        force_resume: cli.force_resume,
    };

    let mut reporter = CliReporter;

    match cli.cmd {
        Commands::Plan => {
            print_plan(&planned);
        }
        Commands::Preflight => {
            let rep = engine::run_preflight(&planned, &opts, &mut reporter)?;
            print_preflight(&rep);
        }
        Commands::Publish => {
            let receipt = engine::run_publish(&planned, &opts, &mut reporter)?;
            print_receipt(&receipt, &planned.workspace_root, &opts.state_dir);
        }
        Commands::Resume => {
            let receipt = engine::run_resume(&planned, &opts, &mut reporter)?;
            print_receipt(&receipt, &planned.workspace_root, &opts.state_dir);
        }
        Commands::Status => {
            run_status(&planned, &mut reporter)?;
        }
        Commands::Doctor => {
            run_doctor(&planned, &opts, &mut reporter)?;
        }
    }

    Ok(())
}

fn parse_duration(s: &str) -> Result<Duration> {
    humantime::parse_duration(s).with_context(|| format!("invalid duration: {s}"))
}

fn print_plan(ws: &plan::PlannedWorkspace) {
    println!("plan_id: {}", ws.plan.plan_id);
    println!(
        "registry: {} ({})",
        ws.plan.registry.name, ws.plan.registry.api_base
    );
    println!("workspace_root: {}", ws.workspace_root.display());
    println!();

    for (idx, p) in ws.plan.packages.iter().enumerate() {
        println!("{:>3}. {}@{}", idx + 1, p.name, p.version);
    }
}

fn print_preflight(rep: &engine::PreflightReport) {
    println!("plan_id: {}", rep.plan_id);
    println!("token_detected: {}", rep.token_detected);
    println!();

    for p in &rep.packages {
        let status = if p.already_published {
            "already published"
        } else {
            "needs publish"
        };
        println!("{}@{}: {status}", p.name, p.version);
    }
}

fn print_receipt(receipt: &shipper::types::Receipt, workspace_root: &PathBuf, state_dir: &PathBuf) {
    println!("plan_id: {}", receipt.plan_id);
    println!(
        "registry: {} ({})",
        receipt.registry.name, receipt.registry.api_base
    );

    let abs_state = if state_dir.is_absolute() {
        state_dir.clone()
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
    println!();

    for p in &receipt.packages {
        println!(
            "{}@{}: {:?} (attempts={}, {}ms)",
            p.name, p.version, p.state, p.attempts, p.duration_ms
        );
    }
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

    let token = shipper::auth::resolve_token(&ws.plan.registry.name)?;
    println!("token_detected: {}", token.is_some());

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

#[cfg(test)]
mod tests {
    use std::env;
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

    fn restore_env(key: &str, value: Option<String>) {
        if let Some(v) = value {
            unsafe { env::set_var(key, v) };
        } else {
            unsafe { env::remove_var(key) };
        }
    }

    #[test]
    fn parse_duration_handles_valid_and_invalid_inputs() {
        assert!(parse_duration("1s").is_ok());
        assert!(parse_duration("nope").is_err());
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
        assert!(reporter
            .warns
            .iter()
            .any(|w| w.contains("--version failed")));
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
                plan_id: "plan-x".to_string(),
                created_at: chrono::Utc::now(),
                registry: Registry::crates_io(),
                packages: vec![],
            },
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
        };

        unsafe { env::set_var("CARGO_REGISTRY_TOKEN", "orig-reg-token") };
        unsafe { env::set_var("CARGO_REGISTRIES_CRATES_IO_TOKEN", "orig-named-token") };
        unsafe { env::remove_var("CARGO_HOME") };

        let old_registry = env::var("CARGO_REGISTRY_TOKEN").ok();
        let old_named = env::var("CARGO_REGISTRIES_CRATES_IO_TOKEN").ok();
        let old_home = env::var("CARGO_HOME").ok();

        unsafe { env::remove_var("CARGO_REGISTRY_TOKEN") };
        unsafe { env::remove_var("CARGO_REGISTRIES_CRATES_IO_TOKEN") };
        unsafe { env::set_var("CARGO_HOME", td.path().join("cargo-home")) };
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let mut reporter = TestReporter::default();
        run_doctor(&ws, &opts, &mut reporter).expect("doctor");

        restore_env("CARGO_REGISTRY_TOKEN", old_registry);
        restore_env("CARGO_REGISTRIES_CRATES_IO_TOKEN", old_named);
        restore_env("CARGO_HOME", old_home);
    }

    #[test]
    #[serial]
    fn run_doctor_restores_env_when_old_values_are_missing_or_present() {
        let td = tempdir().expect("tempdir");
        let ws = plan::PlannedWorkspace {
            workspace_root: td.path().to_path_buf(),
            plan: shipper::types::ReleasePlan {
                plan_id: "plan-y".to_string(),
                created_at: chrono::Utc::now(),
                registry: Registry::crates_io(),
                packages: vec![],
            },
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
        };

        unsafe { env::remove_var("CARGO_REGISTRY_TOKEN") };
        unsafe { env::remove_var("CARGO_REGISTRIES_CRATES_IO_TOKEN") };
        unsafe { env::set_var("CARGO_HOME", td.path().join("orig-home")) };
        fs::create_dir_all(td.path().join("orig-home")).expect("mkdir");

        let old_registry = env::var("CARGO_REGISTRY_TOKEN").ok();
        let old_named = env::var("CARGO_REGISTRIES_CRATES_IO_TOKEN").ok();
        let old_home = env::var("CARGO_HOME").ok();

        unsafe { env::remove_var("CARGO_REGISTRY_TOKEN") };
        unsafe { env::remove_var("CARGO_REGISTRIES_CRATES_IO_TOKEN") };
        unsafe { env::set_var("CARGO_HOME", td.path().join("cargo-home")) };
        fs::create_dir_all(td.path().join("cargo-home")).expect("mkdir");

        let mut reporter = TestReporter::default();
        run_doctor(&ws, &opts, &mut reporter).expect("doctor");

        restore_env("CARGO_REGISTRY_TOKEN", old_registry);
        restore_env("CARGO_REGISTRIES_CRATES_IO_TOKEN", old_named);
        restore_env("CARGO_HOME", old_home);
    }
}
