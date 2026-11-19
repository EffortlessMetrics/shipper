//! Local proof that Changie's retained version files reproduce `CHANGELOG.md`.
//!
//! This is deliberately a maintainer/local command, not a GitHub Actions gate.
//! It executes the pinned Changie binary, renders `changie merge --dry-run`,
//! and compares that output with the tracked changelog while allowing only a
//! final-newline difference.

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CHANGIE_VERSION: &str = "1.25.1";

fn main() -> Result<()> {
    let root = repo_root()?;
    let observed_version = changie_version(&root)?;
    if !changie_version_matches(&observed_version) {
        bail!(
            "expected Changie v{CHANGIE_VERSION}, received `{observed_version}`; align the local tool before validating the baseline"
        );
    }

    let workspace = RoundtripWorkspace::new(&root)?;
    let merge = command_output(&workspace.root, "changie", ["merge", "--dry-run"])
        .context("failed to launch `changie merge --dry-run`")?;
    if !merge.status.success() {
        bail!("Changie merge dry-run failed: {}", output_text(&merge));
    }

    let rendered =
        String::from_utf8(merge.stdout).context("Changie merge output was not valid UTF-8")?;
    let tracked_path = root.join("CHANGELOG.md");
    let tracked = fs::read_to_string(&tracked_path)
        .with_context(|| format!("failed to read {}", tracked_path.display()))?;

    if normalize_final_newline(&rendered) != normalize_final_newline(&tracked) {
        let mismatch = first_mismatch(&rendered, &tracked);
        bail!(
            "Changie round trip does not reproduce CHANGELOG.md. {mismatch}\nRun `changie merge --dry-run`, inspect the retained `.changes/*.md` baseline, and do not run a writing merge until the difference is understood."
        );
    }

    eprintln!(
        "changie round trip: PASS ({observed_version}; {} bytes)",
        normalize_final_newline(&tracked).len()
    );
    Ok(())
}

struct RoundtripWorkspace {
    root: PathBuf,
}

impl RoundtripWorkspace {
    fn new(source_root: &Path) -> Result<Self> {
        let suffix = chrono::Utc::now()
            .timestamp_nanos_opt()
            .context("current time is outside chrono's nanosecond range")?;
        let root = std::env::temp_dir().join(format!(
            "shipper-changie-roundtrip-{suffix}-{}",
            std::process::id()
        ));
        fs::create_dir(&root).with_context(|| format!("failed to create {}", root.display()))?;
        let result = (|| -> Result<()> {
            copy_file(source_root, &root, ".changie.yaml")?;
            copy_file(source_root, &root, "CHANGELOG.md")?;
            copy_directory(&source_root.join(".changes"), &root.join(".changes"), true)?;
            fs::create_dir_all(root.join(".changes/unreleased"))?;
            Ok(())
        })();

        if let Err(error) = result {
            let _ = fs::remove_dir_all(&root);
            return Err(error);
        }

        Ok(Self { root })
    }
}

fn copy_file(source_root: &Path, destination_root: &Path, relative: &str) -> Result<()> {
    let source = source_root.join(relative);
    let destination = destination_root.join(relative);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&source, &destination).with_context(|| {
        format!(
            "failed to copy {} into the Changie round-trip workspace",
            source.display()
        )
    })?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path, skip_unreleased: bool) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry.context("failed to inspect a Changie source entry")?;
        let source_path = entry.path();
        let file_name = entry.file_name();
        if skip_unreleased && file_name == "unreleased" {
            continue;
        }
        let destination_path = destination.join(&file_name);
        let file_type = entry
            .file_type()
            .context("failed to inspect a Changie source entry type")?;
        if file_type.is_dir() {
            copy_directory(&source_path, &destination_path, false)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy {} into the Changie round-trip workspace",
                    source_path.display()
                )
            })?;
        } else {
            bail!(
                "unsupported Changie source entry: {}",
                source_path.display()
            );
        }
    }
    Ok(())
}

impl Drop for RoundtripWorkspace {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!(
                "warning: failed to remove Changie round-trip workspace {}: {error}",
                self.root.display()
            );
        }
    }
}

fn repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to launch git")?;
    if !output.status.success() {
        bail!("not inside a Git worktree: {}", output_text(&output));
    }
    let root = String::from_utf8(output.stdout).context("Git worktree path was not UTF-8")?;
    Ok(PathBuf::from(root.trim()))
}

fn changie_version(root: &Path) -> Result<String> {
    let output = command_output(root, "changie", ["--version"]).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "required executable `changie` was not found; install Changie v{CHANGIE_VERSION} and retry"
            )
        } else {
            anyhow::anyhow!("failed to launch Changie: {error}")
        }
    })?;
    if !output.status.success() {
        bail!("`changie --version` failed: {}", output_text(&output));
    }
    Ok(output_text(&output))
}

fn changie_version_matches(output: &str) -> bool {
    output
        .split_whitespace()
        .map(|token| token.trim_start_matches('v'))
        .any(|token| token == CHANGIE_VERSION)
}

fn normalize_final_newline(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

fn trailing_newline_count(mut text: &str) -> usize {
    let mut count = 0;
    loop {
        if let Some(rest) = text.strip_suffix("\r\n") {
            text = rest;
            count += 1;
        } else if let Some(rest) = text.strip_suffix('\n') {
            text = rest;
            count += 1;
        } else {
            return count;
        }
    }
}

fn first_mismatch(rendered: &str, tracked: &str) -> String {
    let rendered_trailing_newlines = trailing_newline_count(rendered);
    let tracked_trailing_newlines = trailing_newline_count(tracked);
    let rendered = normalize_final_newline(rendered);
    let tracked = normalize_final_newline(tracked);
    let rendered_lines: Vec<&str> = rendered.lines().collect();
    let tracked_lines: Vec<&str> = tracked.lines().collect();
    let shared = rendered_lines.len().min(tracked_lines.len());

    for index in 0..shared {
        if rendered_lines[index] != tracked_lines[index] {
            return format!(
                "First mismatch at line {}: rendered `{}`; tracked `{}`.",
                index + 1,
                rendered_lines[index],
                tracked_lines[index]
            );
        }
    }

    if rendered_lines.len() != tracked_lines.len() {
        return format!(
            "Line-count mismatch after line {shared}: rendered {} lines; tracked {} lines.",
            rendered_lines.len(),
            tracked_lines.len()
        );
    }

    if rendered_trailing_newlines != tracked_trailing_newlines {
        return format!(
            "Trailing-newline mismatch: rendered {rendered_trailing_newlines}; tracked {tracked_trailing_newlines}. Zero or one final newline is permitted, but additional trailing blank lines are not."
        );
    }

    let shared_bytes = rendered.len().min(tracked.len());
    let offset = rendered
        .bytes()
        .zip(tracked.bytes())
        .position(|(left, right)| left != right)
        .unwrap_or(shared_bytes);
    format!(
        "Byte-level mismatch at offset {offset}: rendered {} bytes; tracked {} bytes.",
        rendered.len(),
        tracked.len()
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_match_is_exact() {
        assert!(changie_version_matches("changie version v1.25.1"));
        assert!(changie_version_matches("v1.25.1"));
        assert!(!changie_version_matches("changie version v1.25.10"));
        assert!(!changie_version_matches("changie version vdev"));
    }

    #[test]
    fn only_the_final_newline_is_normalized() {
        assert_eq!(normalize_final_newline("body\n"), "body");
        assert_eq!(normalize_final_newline("body\r\n"), "body");
        assert_eq!(normalize_final_newline("body\n\n"), "body\n");
        assert_ne!(normalize_final_newline("body "), "body");
    }

    #[test]
    fn trailing_newlines_are_counted_by_logical_line_ending() {
        assert_eq!(trailing_newline_count("body"), 0);
        assert_eq!(trailing_newline_count("body\n"), 1);
        assert_eq!(trailing_newline_count("body\r\n\r\n"), 2);
    }

    #[test]
    fn mismatch_reports_the_first_different_line() {
        assert_eq!(
            first_mismatch("one\ntwo\n", "one\nthree"),
            "First mismatch at line 2: rendered `two`; tracked `three`."
        );
    }

    #[test]
    fn mismatch_reports_missing_history() {
        assert_eq!(
            first_mismatch("one\n", "one\ntwo\n"),
            "Line-count mismatch after line 1: rendered 1 lines; tracked 2 lines."
        );
    }

    #[test]
    fn mismatch_reports_additional_trailing_blank_lines() {
        assert_eq!(
            first_mismatch("one\n", "one\n\n"),
            "Trailing-newline mismatch: rendered 1; tracked 2. Zero or one final newline is permitted, but additional trailing blank lines are not."
        );
    }
}
