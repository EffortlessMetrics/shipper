//! Repository-owned local pre-commit gate for staged Changie hygiene.
//!
//! The gate is deliberately local-only. It validates the Git index, requires a
//! branch-local Changie fragment for release-note-relevant changes, validates
//! Changie configuration/fragments with the pinned executable, and writes an
//! advisory receipt. It is not a GitHub/merge gate and is intentionally not
//! duplicated in CI.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::json;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const RECEIPT_SCHEMA_VERSION: &str = "shipper.precommit.v1";
const CHANGIE_VERSION: &str = "1.25.1";
const CHANGIE_VALIDATION_VERSION: &str = "9999.0.0-precommit";
const DEFAULT_BASE_REF: &str = "origin/main";
const CHANGELOG_EXEMPT_ENV: &str = "SHIPPER_PRECOMMIT_CHANGELOG_EXEMPT";
const PRECOMMIT_HOOK_ENV: &str = "SHIPPER_PRECOMMIT_HOOK";
const PRECOMMIT_REPO_ROOT_ENV: &str = "SHIPPER_PRECOMMIT_REPO_ROOT";
const HOOK_MARKER: &str = "# shipper-swarm pre-commit hook v2";
const OWNED_HOOK_PREFIX: &str = "# shipper-swarm pre-commit hook v";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookState {
    Missing,
    Current,
    Stale,
    Conflicting,
}

struct Snapshot {
    path: PathBuf,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct ReceiptInput<'a> {
    base_ref: &'a str,
    base_ref_available: bool,
    staged: &'a [String],
    release_note_paths: &'a [String],
    fragment_paths: &'a [String],
    changelog_exemption: Option<&'a str>,
    changie_required: bool,
    changie_allow_no_changes: bool,
    changie_validated: bool,
    changie_version: Option<&'a str>,
    failures: &'a [String],
    overall: bool,
}

/// Validate the staged index and its branch-local Changie disposition.
pub(crate) fn run() -> Result<()> {
    let root = repo_root()?;
    run_at(&root)
}

/// Install the repository-owned `pre-commit` dispatcher.
pub(crate) fn install() -> Result<()> {
    let root = repo_root()?;
    let path = hook_path(&root)?;

    match hook_state(&path)? {
        HookState::Current => {
            eprintln!("pre-commit hook is already current: {}", path.display());
            return Ok(());
        }
        HookState::Conflicting => {
            bail!(
                "refusing to overwrite a non-Shipper or modified current-version pre-commit hook at {}; move or explicitly chain it first",
                path.display()
            );
        }
        HookState::Missing | HookState::Stale => {}
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create hook directory {}", parent.display()))?;
    }

    let temporary = path.with_extension(format!("shipper-tmp-{}", std::process::id()));
    fs::write(&temporary, hook_script())
        .with_context(|| format!("failed to write temporary hook {}", temporary.display()))?;
    make_executable(&temporary)?;

    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove stale hook {}", path.display()))?;
    }
    fs::rename(&temporary, &path).with_context(|| {
        format!(
            "failed to move temporary hook {} into {}",
            temporary.display(),
            path.display()
        )
    })?;

    eprintln!("installed Shipper pre-commit hook: {}", path.display());
    Ok(())
}

/// Report whether the effective pre-commit hook is the current Shipper hook.
pub(crate) fn status() -> Result<()> {
    let root = repo_root()?;
    let path = hook_path(&root)?;
    let state = hook_state(&path)?;
    eprintln!("pre-commit hook: {state:?} ({})", path.display());
    if state == HookState::Current {
        Ok(())
    } else {
        bail!("run `cargo precommit install` to install the repository-owned hook")
    }
}

/// Remove only a Shipper-owned current or stale pre-commit hook.
pub(crate) fn uninstall() -> Result<()> {
    let root = repo_root()?;
    let path = hook_path(&root)?;
    match hook_state(&path)? {
        HookState::Missing => {
            eprintln!("no pre-commit hook is installed: {}", path.display());
            Ok(())
        }
        HookState::Current | HookState::Stale => {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove hook {}", path.display()))?;
            eprintln!("removed Shipper pre-commit hook: {}", path.display());
            Ok(())
        }
        HookState::Conflicting => bail!(
            "refusing to remove a non-Shipper or modified current-version pre-commit hook at {}",
            path.display()
        ),
    }
}

fn run_at(root: &Path) -> Result<()> {
    let staged = staged_paths(root)?;
    let indexed_symlinks = indexed_symlink_paths(root)?;
    if !indexed_symlinks.is_empty() {
        bail!(
            "pre-commit refuses to execute Cargo or Changie from a staged index containing symbolic links because a link can escape the private snapshot: {}; replace the links with regular files or land a separately reviewed containment policy before retrying",
            indexed_symlinks.join(", ")
        );
    }

    let base_ref =
        env::var("SHIPPER_PRECOMMIT_BASE").unwrap_or_else(|_| DEFAULT_BASE_REF.to_string());
    let branch_paths = changed_paths_from_base(root, &base_ref)?;
    let base_ref_available = branch_paths.is_some();
    let branch_paths = branch_paths.unwrap_or_default();
    let changelog_exemption = changelog_exemption()?;

    let release_note_paths: Vec<String> = staged
        .iter()
        .filter(|path| is_release_note_relevant(path))
        .cloned()
        .collect();
    let changie_surface_changed = staged.iter().any(|path| is_changie_surface(path));
    let changie_required = !release_note_paths.is_empty() || changie_surface_changed;

    let mut failures = Vec::new();
    let diff_check = command_output(root, "git", ["diff", "--cached", "--check"])
        .context("failed to run staged diff hygiene check")?;
    if !diff_check.status.success() {
        failures.push(format!(
            "staged diff hygiene failed: {}",
            output_text(&diff_check)
        ));
    }

    let snapshot = match create_staged_snapshot(root) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            failures.push(format!("failed to materialize the staged index: {error:#}"));
            None
        }
    };

    let mut fragment_candidates = staged.clone();
    fragment_candidates.extend(branch_paths.iter().cloned());
    fragment_candidates.sort();
    fragment_candidates.dedup();

    let fragment_paths = snapshot
        .as_ref()
        .map(|snapshot| {
            fragment_candidates
                .iter()
                .filter(|path| is_unreleased_fragment(path))
                .filter(|path| snapshot.path.join(path).is_file())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if !release_note_paths.is_empty() && fragment_paths.is_empty() && changelog_exemption.is_none()
    {
        failures.push(format!(
            "release-note-relevant staged paths require a branch-local Changie fragment: {}; run `changie new`, stage `.changes/unreleased/*.yaml`, and retry. For a genuinely non-user-facing exception, set {CHANGELOG_EXEMPT_ENV} to a substantive reason of at least 12 characters for this commit",
            release_note_paths.join(", ")
        ));
    }

    let changie_allow_no_changes = allows_empty_changie_batch(&release_note_paths, &fragment_paths);
    let mut changie_version = None;
    let mut changie_validated = false;
    if changie_required && (changelog_exemption.is_none() || changie_surface_changed) {
        match snapshot.as_ref() {
            Some(snapshot) => match validate_changie(&snapshot.path, changie_allow_no_changes) {
                Ok(version) => {
                    changie_version = Some(version);
                    changie_validated = true;
                }
                Err(error) => failures.push(error.to_string()),
            },
            None => failures.push(
                "Changie validation could not run because the staged snapshot was unavailable"
                    .to_string(),
            ),
        }
    }

    let overall = failures.is_empty();
    let receipt = ReceiptInput {
        base_ref: &base_ref,
        base_ref_available,
        staged: &staged,
        release_note_paths: &release_note_paths,
        fragment_paths: &fragment_paths,
        changelog_exemption: changelog_exemption.as_deref(),
        changie_required,
        changie_allow_no_changes,
        changie_validated,
        changie_version: changie_version.as_deref(),
        failures: &failures,
        overall,
    };
    write_receipt(root, &receipt)?;

    eprintln!("pre-commit staged paths: {}", staged.len());
    eprintln!("release-note-relevant paths: {}", release_note_paths.len());
    eprintln!("branch-local fragments: {}", fragment_paths.len());
    if let Some(reason) = changelog_exemption.as_deref() {
        eprintln!("changelog exemption: {reason}");
    }
    if !base_ref_available {
        eprintln!(
            "note: `{base_ref}` was unavailable; only fragments present in the staged index could satisfy the check"
        );
    }

    if overall {
        eprintln!("pre-commit: PASS");
        return Ok(());
    }

    eprintln!("pre-commit: FAIL");
    for failure in &failures {
        eprintln!("  - {failure}");
    }
    bail!("staged pre-commit checks failed")
}

fn changelog_exemption() -> Result<Option<String>> {
    parse_changelog_exemption(env::var_os(CHANGELOG_EXEMPT_ENV).as_deref())
}

fn parse_changelog_exemption(raw: Option<&OsStr>) -> Result<Option<String>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let reason = raw.to_string_lossy().trim().to_string();
    if reason.len() < 12 {
        bail!("{CHANGELOG_EXEMPT_ENV} must contain a substantive reason of at least 12 characters");
    }
    Ok(Some(reason))
}

fn allows_empty_changie_batch(release_note_paths: &[String], fragment_paths: &[String]) -> bool {
    release_note_paths.is_empty() && fragment_paths.is_empty()
}

fn validate_changie(snapshot: &Path, allow_no_changes: bool) -> Result<String> {
    let version_output = command_output(snapshot, "changie", ["--version"]).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "required executable `changie` was not found; install Changie v{CHANGIE_VERSION} and retry"
            )
        } else {
            anyhow::anyhow!("failed to launch Changie: {error}")
        }
    })?;
    if !version_output.status.success() {
        bail!(
            "`changie --version` failed: {}",
            output_text(&version_output)
        );
    }

    let version = output_text(&version_output);
    if !changie_version_matches(&version) {
        bail!(
            "expected Changie v{CHANGIE_VERSION}, received `{version}`; align the local tool before committing"
        );
    }

    let allow_no_changes = if allow_no_changes { "true" } else { "false" };
    let batch_output = command_output(
        snapshot,
        "changie",
        [
            "batch",
            CHANGIE_VALIDATION_VERSION,
            "--dry-run",
            &format!("--allow-no-changes={allow_no_changes}"),
        ],
    )
    .context("failed to launch Changie dry-run validation")?;
    if !batch_output.status.success() {
        bail!(
            "Changie configuration or fragment validation failed: {}",
            output_text(&batch_output)
        );
    }

    Ok(version)
}

fn changie_version_matches(output: &str) -> bool {
    output
        .split_whitespace()
        .map(|token| token.trim_start_matches('v'))
        .any(|token| token == CHANGIE_VERSION)
}

fn write_receipt(root: &Path, input: &ReceiptInput<'_>) -> Result<()> {
    let head = git_text(root, &["rev-parse", "HEAD"]).unwrap_or_else(|_| "unborn".to_string());
    let receipt = json!({
        "schema_version": RECEIPT_SCHEMA_VERSION,
        "generated_at": Utc::now().to_rfc3339(),
        "hook": "pre-commit",
        "invocation": if env::var_os(PRECOMMIT_HOOK_ENV).is_some() { "git-hook" } else { "manual" },
        "head": head,
        "base_ref": input.base_ref,
        "base_ref_available": input.base_ref_available,
        "staged_files": input.staged,
        "release_note_relevant_files": input.release_note_paths,
        "branch_local_fragments": input.fragment_paths,
        "changelog_exemption": input.changelog_exemption,
        "changie": {
            "required": input.changie_required,
            "allow_no_changes": input.changie_allow_no_changes,
            "validated": input.changie_validated,
            "expected_version": CHANGIE_VERSION,
            "observed_version": input.changie_version,
        },
        "failures": input.failures,
        "overall": if input.overall { "pass" } else { "fail" },
        "claim_boundary": "This local receipt proves staged-index hygiene and Changie authoring checks only. Git hooks are bypassable and are intentionally not merge authority or a CI gate."
    });

    let output_dir = root.join("target/hooks");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let output_path = output_dir.join("pre-commit.json");
    let bytes =
        serde_json::to_vec_pretty(&receipt).context("failed to encode pre-commit receipt")?;
    fs::write(&output_path, bytes)
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    Ok(())
}

fn repo_root() -> Result<PathBuf> {
    let hook_invocation = env::var_os(PRECOMMIT_HOOK_ENV).is_some();
    match (hook_invocation, env::var_os(PRECOMMIT_REPO_ROOT_ENV)) {
        (true, Some(root)) => validate_repo_root(Path::new(&root)),
        (true, None) => {
            bail!("{PRECOMMIT_REPO_ROOT_ENV} is required when {PRECOMMIT_HOOK_ENV} is set")
        }
        (false, Some(_)) => bail!(
            "{PRECOMMIT_REPO_ROOT_ENV} is only honored for the repository-owned installed hook"
        ),
        (false, None) => discover_repo_root(None),
    }
}

fn discover_repo_root(cwd: Option<&Path>) -> Result<PathBuf> {
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to launch git")?;
    if !output.status.success() {
        bail!("not inside a Git worktree: {}", output_text(&output));
    }
    let root = String::from_utf8(output.stdout).context("Git worktree path was not UTF-8")?;
    Ok(PathBuf::from(root.trim()))
}

fn validate_repo_root(candidate: &Path) -> Result<PathBuf> {
    let discovered = discover_repo_root(Some(candidate))?;
    let canonical_candidate = fs::canonicalize(candidate).with_context(|| {
        format!(
            "failed to canonicalize {PRECOMMIT_REPO_ROOT_ENV} candidate {}",
            candidate.display()
        )
    })?;
    let canonical_discovered = fs::canonicalize(&discovered).with_context(|| {
        format!(
            "failed to canonicalize discovered repository root {}",
            discovered.display()
        )
    })?;
    if canonical_candidate != canonical_discovered {
        bail!(
            "{PRECOMMIT_REPO_ROOT_ENV} must name the repository top level exactly: candidate {}; discovered {}",
            candidate.display(),
            discovered.display()
        );
    }
    Ok(discovered)
}

fn staged_paths(root: &Path) -> Result<Vec<String>> {
    let output = command_output(
        root,
        "git",
        [
            "-c",
            "core.quotepath=false",
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=ACMRDTUXB",
        ],
    )
    .context("failed to enumerate staged paths")?;
    if !output.status.success() {
        bail!("failed to enumerate staged paths: {}", output_text(&output));
    }
    parse_nul_paths(&output.stdout)
}

fn indexed_symlink_paths(root: &Path) -> Result<Vec<String>> {
    let output = command_output(
        root,
        "git",
        ["-c", "core.quotepath=false", "ls-files", "-s", "-z"],
    )
    .context("failed to enumerate staged index modes")?;
    if !output.status.success() {
        bail!(
            "failed to enumerate staged index modes: {}",
            output_text(&output)
        );
    }
    parse_index_symlink_paths(&output.stdout)
}

fn parse_index_symlink_paths(bytes: &[u8]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for record in bytes.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            bail!("Git index record did not contain a path separator")
        };
        let metadata = &record[..tab];
        let mode = metadata
            .split(|byte| *byte == b' ')
            .next()
            .unwrap_or_default();
        if mode == b"120000" {
            paths.push(
                String::from_utf8(record[tab + 1..].to_vec())
                    .context("Git symlink path was not UTF-8")?,
            );
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn changed_paths_from_base(root: &Path, base_ref: &str) -> Result<Option<Vec<String>>> {
    let verify_ref = format!("{base_ref}^{{commit}}");
    let verify = command_output(root, "git", ["rev-parse", "--verify", verify_ref.as_str()])
        .context("failed to verify the pre-commit base ref")?;
    if !verify.status.success() {
        return Ok(None);
    }

    let range = format!("{base_ref}...HEAD");
    let output = command_output(
        root,
        "git",
        [
            "-c",
            "core.quotepath=false",
            "diff",
            "--name-only",
            "-z",
            "--diff-filter=ACMRD",
            range.as_str(),
        ],
    )
    .context("failed to enumerate branch paths")?;
    if !output.status.success() {
        bail!(
            "failed to enumerate branch paths from `{base_ref}`: {}",
            output_text(&output)
        );
    }
    parse_nul_paths(&output.stdout).map(Some)
}

fn parse_nul_paths(bytes: &[u8]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for raw in bytes.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        paths.push(String::from_utf8(raw.to_vec()).context("Git path was not UTF-8")?);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn create_staged_snapshot(root: &Path) -> Result<Snapshot> {
    let path = create_private_snapshot_dir()?;

    let mut prefix = path.to_string_lossy().replace('\\', "/");
    if !prefix.ends_with('/') {
        prefix.push('/');
    }
    let output = Command::new("git")
        .current_dir(root)
        .args(["checkout-index", "--all", "--force"])
        .arg(format!("--prefix={prefix}"))
        .output()
        .context("failed to launch git checkout-index")?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&path);
        bail!(
            "failed to materialize staged snapshot: {}",
            output_text(&output)
        );
    }

    Ok(Snapshot { path })
}

fn create_private_snapshot_dir() -> Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let parent = env::temp_dir();

    for attempt in 0..100_u8 {
        let path = parent.join(format!(
            "shipper-precommit-{}-{nonce}-{attempt}",
            std::process::id()
        ));
        match create_private_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create private snapshot directory {}",
                        path.display()
                    )
                });
            }
        }
    }

    bail!("failed to allocate a unique private snapshot directory")
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

fn is_unreleased_fragment(path: &str) -> bool {
    path.starts_with(".changes/unreleased/") && path.ends_with(".yaml")
}

fn is_changie_surface(path: &str) -> bool {
    path == ".changie.yaml" || path == "CHANGELOG.md" || path.starts_with(".changes/")
}

fn is_release_note_relevant(path: &str) -> bool {
    if matches!(
        path,
        "README.md"
            | "Cargo.toml"
            | "rust-toolchain.toml"
            | ".github/workflows/release.yml"
            | "docs/INVARIANTS.md"
            | "docs/README.md"
            | "docs/configuration.md"
            | "docs/failure-modes.md"
            | "docs/preflight.md"
            | "docs/product.md"
            | "docs/readiness.md"
            | "docs/release-runbook.md"
    ) {
        return true;
    }

    if path.starts_with("templates/")
        || path.starts_with("docs/how-to/")
        || path.starts_with("docs/tutorials/")
        || path.starts_with("docs/reference/")
        || path.starts_with("docs/explanation/")
    {
        return true;
    }

    if !path.starts_with("crates/") {
        return false;
    }
    if is_test_artifact(path) {
        return false;
    }
    if path.ends_with("/Cargo.toml") || path.ends_with("/README.md") {
        return true;
    }
    path.contains("/src/")
}

fn is_test_artifact(path: &str) -> bool {
    path.contains("/tests/")
        || path.contains("/benches/")
        || path.contains("/examples/")
        || path.contains("/snapshots/")
        || path.contains("/proptest-regressions/")
        || path.ends_with("/tests.rs")
        || path.ends_with("_tests.rs")
        || path.ends_with(".snap")
}

fn command_output<I, S>(cwd: &Path, program: &str, args: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program).current_dir(cwd).args(args).output()
}

fn output_text(output: &Output) -> String {
    let mut text = String::new();
    if !output.stdout.is_empty() {
        text.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    text.trim().to_string()
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output =
        command_output(root, "git", args.iter().copied()).context("failed to launch git")?;
    if !output.status.success() {
        bail!("git {} failed: {}", args.join(" "), output_text(&output));
    }
    String::from_utf8(output.stdout)
        .context("Git output was not UTF-8")
        .map(|text| text.trim().to_string())
}

fn hook_script() -> String {
    format!(
        "#!/bin/sh\n{HOOK_MARKER}\nset -eu\nrepo_root=$(git rev-parse --show-toplevel)\nif git -C \"$repo_root\" ls-files -s | grep -q '^120000 '; then\n  echo 'pre-commit: symbolic links in the staged index are not supported because they can escape the private snapshot' >&2\n  exit 1\nfi\nsnapshot=$(mktemp -d \"${{TMPDIR:-/tmp}}/shipper-precommit-hook.XXXXXX\")\ncleanup() {{\n  rm -rf \"$snapshot\"\n}}\ntrap 'status=$?; trap - EXIT HUP INT TERM; cleanup || true; exit \"$status\"' EXIT\ntrap 'exit 129' HUP\ntrap 'exit 130' INT\ntrap 'exit 143' TERM\nprefix=\"$snapshot/\"\ngit -C \"$repo_root\" checkout-index --all --force --prefix=\"$prefix\"\ncd \"$snapshot\"\nexport {PRECOMMIT_HOOK_ENV}=1\nexport {PRECOMMIT_REPO_ROOT_ENV}=\"$repo_root\"\nexport CARGO_TARGET_DIR=\"$repo_root/target/precommit-staged\"\ncargo run --locked --manifest-path \"$snapshot/xtask/Cargo.toml\" --package xtask --bin xtask -- precommit run\n"
    )
}

fn hook_state_from_text(text: Option<&str>) -> HookState {
    let expected = hook_script();
    match text {
        None => HookState::Missing,
        Some(text) if text == expected.as_str() => HookState::Current,
        Some(text) if text.lines().any(|line| line.trim() == HOOK_MARKER) => HookState::Conflicting,
        Some(text)
            if text
                .lines()
                .any(|line| line.trim().starts_with(OWNED_HOOK_PREFIX)) =>
        {
            HookState::Stale
        }
        Some(_) => HookState::Conflicting,
    }
}

fn hook_path(root: &Path) -> Result<PathBuf> {
    let raw = git_text(root, &["rev-parse", "--git-path", "hooks/pre-commit"])?;
    let path = PathBuf::from(raw);
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn hook_state(path: &Path) -> Result<HookState> {
    if !path.exists() {
        return Ok(HookState::Missing);
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read hook {}", path.display()))?;
    let state = hook_state_from_text(Some(&text));
    if state == HookState::Current && !is_executable(path)? {
        Ok(HookState::Stale)
    } else {
        Ok(state)
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)
        .with_context(|| format!("failed to stat hook {}", path.display()))?
        .permissions()
        .mode()
        & 0o111
        != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> Result<bool> {
    Ok(true)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to stat hook {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to mark hook executable: {}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_V1_HOOK_SCRIPT: &str = "#!/bin/sh\n# shipper-swarm pre-commit hook v1\nset -eu\nrepo_root=$(git rev-parse --show-toplevel)\ncd \"$repo_root\"\nexport SHIPPER_PRECOMMIT_HOOK=1\nexec cargo precommit run\n";

    #[test]
    fn release_note_paths_cover_product_and_user_documentation() {
        assert!(is_release_note_relevant(
            "crates/shipper-core/src/engine/mod.rs"
        ));
        assert!(is_release_note_relevant("crates/shipper/Cargo.toml"));
        assert!(is_release_note_relevant("crates/shipper/README.md"));
        assert!(is_release_note_relevant(
            "docs/how-to/publish-missing-workspace-crates.md"
        ));
        assert!(is_release_note_relevant("docs/README.md"));
        assert!(is_release_note_relevant("docs/release-runbook.md"));
        assert!(is_release_note_relevant(".github/workflows/release.yml"));
        assert!(is_release_note_relevant("rust-toolchain.toml"));
    }

    #[test]
    fn test_policy_and_internal_surfaces_are_automatically_exempt() {
        assert!(!is_release_note_relevant(
            "crates/shipper-core/tests/publish.rs"
        ));
        assert!(!is_release_note_relevant(
            "crates/shipper-core/tests/fixtures/example/Cargo.toml"
        ));
        assert!(!is_release_note_relevant(
            "crates/shipper-core/src/engine/tests.rs"
        ));
        assert!(!is_release_note_relevant(
            "crates/shipper-cli/tests/snapshots/help.snap"
        ));
        assert!(!is_release_note_relevant("policy/non-rust-allowlist.toml"));
        assert!(!is_release_note_relevant("docs/status/SUPPORT_TIERS.md"));
        assert!(!is_release_note_relevant(".github/workflows/ci.yml"));
        assert!(!is_release_note_relevant("xtask/src/precommit.rs"));
    }

    #[test]
    fn only_unreleased_yaml_files_are_fragments() {
        assert!(is_unreleased_fragment(
            ".changes/unreleased/fixed-20260804-120000.yaml"
        ));
        assert!(!is_unreleased_fragment(".changes/unreleased/.gitkeep"));
        assert!(!is_unreleased_fragment(".changes/0.5.0.md"));
        assert!(!is_unreleased_fragment("docs/change.yaml"));
    }

    #[test]
    fn hook_ownership_is_fail_closed() {
        let current = hook_script();
        assert_eq!(hook_state_from_text(None), HookState::Missing);
        assert_eq!(
            hook_state_from_text(Some(current.as_str())),
            HookState::Current
        );
        assert_eq!(
            hook_state_from_text(Some(LEGACY_V1_HOOK_SCRIPT)),
            HookState::Stale
        );
        assert_eq!(
            hook_state_from_text(Some("#!/bin/sh\n# shipper-swarm pre-commit hook v2\n")),
            HookState::Conflicting
        );
        assert_eq!(
            hook_state_from_text(Some("#!/bin/sh\nexec foreign-tool\n")),
            HookState::Conflicting
        );
    }

    #[test]
    fn installed_hook_dispatches_to_staged_sources() {
        let script = hook_script();
        assert!(script.contains(HOOK_MARKER));
        assert!(script.contains("ls-files -s"));
        assert!(script.contains("^120000 "));
        assert!(script.contains("git -C \"$repo_root\" checkout-index"));
        assert!(script.contains("--manifest-path \"$snapshot/xtask/Cargo.toml\""));
        assert!(script.contains("--package xtask --bin xtask -- precommit run"));
        assert!(script.contains(PRECOMMIT_REPO_ROOT_ENV));
        assert!(script.contains(PRECOMMIT_HOOK_ENV));
        assert!(script.contains("cleanup || true"));
        assert!(script.contains("status=$?"));
        assert!(!script.contains("exec cargo precommit run"));
    }

    #[test]
    fn index_symlink_records_are_detected() -> Result<()> {
        let records = b"100644 1111111111111111111111111111111111111111 0\tCargo.toml\0\
120000 2222222222222222222222222222222222222222 0\tlinked docs/guide.md\0";
        assert_eq!(
            parse_index_symlink_paths(records)?,
            vec!["linked docs/guide.md".to_string()]
        );
        Ok(())
    }

    #[test]
    fn changelog_exemption_requires_a_substantive_reason() -> Result<()> {
        assert_eq!(parse_changelog_exemption(None)?, None);
        assert!(parse_changelog_exemption(Some(OsStr::new("too short"))).is_err());
        assert_eq!(
            parse_changelog_exemption(Some(OsStr::new("test-only inline module")))?,
            Some("test-only inline module".to_string())
        );
        Ok(())
    }

    #[test]
    fn changie_version_match_is_exact() {
        assert!(changie_version_matches("changie version v1.25.1"));
        assert!(changie_version_matches("v1.25.1"));
        assert!(!changie_version_matches("changie version v1.25.10"));
        assert!(!changie_version_matches("changie version vdev"));
    }

    #[test]
    fn empty_changie_batch_is_only_allowed_without_release_note_paths_or_fragments() {
        let no_paths: Vec<String> = Vec::new();
        let product_paths = ["crates/shipper-core/src/lib.rs".to_string()];
        let fragment_paths =
            [".changes/unreleased/fixed-20260804-120000.000000000.yaml".to_string()];

        assert!(allows_empty_changie_batch(&no_paths, &no_paths));
        assert!(!allows_empty_changie_batch(&product_paths, &no_paths));
        assert!(!allows_empty_changie_batch(&no_paths, &fragment_paths));
        assert!(!allows_empty_changie_batch(&product_paths, &fragment_paths));
    }

    #[test]
    fn repository_override_must_name_the_toplevel_exactly() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("xtask must live directly under the repository root")?;
        let validated = validate_repo_root(root)?;
        assert_eq!(fs::canonicalize(validated)?, fs::canonicalize(root)?);
        assert!(validate_repo_root(&root.join("xtask")).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn staged_snapshot_directory_is_owner_only() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let path = create_private_snapshot_dir()?;
        let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
        fs::remove_dir(&path)?;
        assert_eq!(mode, 0o700);
        Ok(())
    }
}
