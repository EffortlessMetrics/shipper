//! Receipt source evidence follows the selected workspace, never the caller CWD.

use std::path::PathBuf;
use std::process::Command as StdCommand;

use serde_json::{Value, json};
use tempfile::{TempDir, tempdir_in};

use super::*;

const REPOSITORY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_PREFIX",
    "GIT_GRAFT_FILE",
    "GIT_SHALLOW_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_NAMESPACE",
];

fn git(root: &Path, args: &[&str]) -> String {
    let mut command = StdCommand::new("git");
    command.args(args).current_dir(root);
    // Fixture writes must never target a hook's or test runner's repository.
    for variable in REPOSITORY_ENV {
        command.env_remove(variable);
    }
    // In particular, GIT_CONFIG can redirect fixture `git config` writes.
    command
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT");
    let output = command.output().expect("run fixture Git command");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("Git output is UTF-8")
        .trim()
        .to_owned()
}

fn init_git(root: &Path, branch: &str) -> String {
    git(root, &["init", "-q", "-b", branch]);
    git(root, &["config", "user.name", "Receipt Fixture"]);
    git(root, &["config", "user.email", "receipt@example.invalid"]);
    git(root, &["add", "."]);
    git(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", branch],
    );
    git(
        root,
        &["-c", "tag.gpgSign=false", "tag", &format!("{branch}-tag")],
    );
    git(root, &["rev-parse", "HEAD"])
}

struct Fixture {
    root: TempDir,
    target: PathBuf,
    caller: PathBuf,
    commit: String,
    new_path: String,
    real_cargo: String,
    fake_cargo: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempdir().expect("fixture directory");
        let target = root.path().join("target-workspace");
        create_single_crate_workspace(&target);
        write_file(&target.join(".gitignore"), "Cargo.lock\n/target/\n");
        let commit = init_git(&target, "target");

        let caller = root.path().join("caller-repository");
        write_file(&caller.join("README"), "a different repository\n");
        let caller_commit = init_git(&caller, "caller");
        assert_ne!(commit, caller_commit, "distinct source identities");
        write_file(&caller.join("untracked"), "caller is dirty\n");

        let (new_path, real_cargo, fake_cargo) = setup_fake_cargo(root.path());
        Self {
            root,
            target,
            caller,
            commit,
            new_path,
            real_cargo,
            fake_cargo,
        }
    }

    fn publish_and_resume(
        &self,
        target: &Path,
        caller: &Path,
        flags: &[&str],
        git_override: Option<&Path>,
    ) -> Result<Vec<Value>> {
        self.publish_and_resume_with_git_env(target, caller, flags, git_override, &[])
    }

    fn command(&self, target: &Path, caller: &Path, state: &Path, api_base: &str) -> Command {
        let mut command = loopback_shipper_cmd();
        command
            .timeout(Duration::from_secs(15))
            .current_dir(caller)
            .arg("--manifest-path")
            .arg(target.join("Cargo.toml"))
            .arg("--api-base")
            .arg(api_base)
            .arg("--state-dir")
            .arg(state)
            .args([
                "--policy",
                "fast",
                "--verify-mode",
                "none",
                "--no-readiness",
                "--skip-ownership-check",
            ])
            .env("PATH", &self.new_path)
            .env("REAL_CARGO", &self.real_cargo)
            .env("CARGO_NET_OFFLINE", "true")
            .env("SHIPPER_CARGO_BIN", &self.fake_cargo)
            .env("SHIPPER_FAKE_PUBLISH_LOG", state.join("cargo-publish.log"))
            .env("SHIPPER_FAKE_PUBLISH_EXIT", "71")
            .env("SHIPPER_TEST_GIT_LOG", self.root.path().join("git-calls"))
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "shipper.receiptMarker")
            .env("GIT_CONFIG_VALUE_0", "preserved")
            .env_remove("SHIPPER_GIT_BIN");
        // Start isolated; conflicting settings are applied deliberately below.
        for variable in REPOSITORY_ENV {
            command.env_remove(variable);
        }
        command
    }

    fn publish_and_resume_with_git_env(
        &self,
        target: &Path,
        caller: &Path,
        flags: &[&str],
        git_override: Option<&Path>,
        git_env: &[(&str, &Path)],
    ) -> Result<Vec<Value>> {
        let state = tempdir_in(self.root.path())?;
        let publish_log = state.path().join("cargo-publish.log");
        // The first process sees the version already published. The second
        // trusts its terminal state; neither may invoke Cargo publication.
        let registry = spawn_bounded_registry(vec![200], 1);
        let result = (|| -> Result<Vec<Value>> {
            let mut contexts = Vec::new();
            for operation in ["publish", "resume"] {
                let mut command = self.command(target, caller, state.path(), &registry.base_url);
                command.args(flags).arg(operation);
                if let Some(program) = git_override {
                    command.env("SHIPPER_GIT_BIN", program);
                }
                for (variable, value) in git_env {
                    command.env(variable, value);
                }
                let output = command.output()?;
                ensure!(
                    output.status.success(),
                    "{operation} from {}: {}",
                    caller.display(),
                    String::from_utf8_lossy(&output.stderr)
                );
                let receipt: Value =
                    serde_json::from_slice(&fs::read(state.path().join("receipt.json"))?)?;
                ensure!(receipt["execution_result"] == "success");
                contexts.push(receipt["git_context"].clone());
            }
            Ok(contexts)
        })();
        // Join even when the subprocess or receipt assertions failed.
        let registry_result = registry.finish(Duration::from_secs(2));
        let contexts = result?;
        registry_result?;
        ensure!(
            read_publish_log(&publish_log).is_empty(),
            "already-published fixture invoked Cargo publication"
        );
        Ok(contexts)
    }

    fn assert_dirty_target_rejected(
        &self,
        flags: &[&str],
        git_override: Option<&Path>,
        git_env: &[(&str, &Path)],
    ) -> Result<()> {
        let state = tempdir_in(self.root.path())?;
        let registry = spawn_bounded_registry(Vec::new(), 0);
        let mut command =
            self.command(&self.target, &self.caller, state.path(), &registry.base_url);
        command.args(flags).arg("publish");
        if let Some(program) = git_override {
            command.env("SHIPPER_GIT_BIN", program);
        }
        for (variable, value) in git_env {
            command.env(variable, value);
        }
        let output = command.output();
        let registry_result = registry.finish(Duration::from_secs(2));
        let output = output?;
        registry_result?;
        ensure!(!output.status.success(), "dirty workspace was accepted");
        ensure!(
            String::from_utf8_lossy(&output.stderr).contains("git working tree is not clean"),
            "wrong refusal: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            !state.path().join("state.json").exists()
                && !state.path().join("receipt.json").exists(),
            "dirty workspace reached execution state or receipt creation"
        );
        ensure!(read_publish_log(&state.path().join("cargo-publish.log")).is_empty());
        Ok(())
    }

    fn git_proxy(&self) -> Result<PathBuf> {
        #[cfg(windows)]
        let program = {
            let path = self.root.path().join("git-proxy.cmd");
            fs::write(
                &path,
                "@echo off\r\ngit config --get shipper.receiptMarker | findstr /x preserved >nul\r\nif errorlevel 1 exit /b 73\r\necho %CD%>>\"%SHIPPER_TEST_GIT_LOG%\"\r\ngit %*\r\nexit /b %ERRORLEVEL%\r\n",
            )?;
            path
        };
        #[cfg(not(windows))]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let path = self.root.path().join("git-proxy");
            fs::write(
                &path,
                "#!/bin/sh\n[ \"$(git config --get shipper.receiptMarker)\" = preserved ] || exit 73\npwd >> \"$SHIPPER_TEST_GIT_LOG\"\nexec git \"$@\"\n",
            )?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            path
        };
        Ok(program)
    }

    fn expected_context(&self, branch: Option<&str>, dirty: bool) -> Value {
        json!({
            "commit": self.commit,
            "branch": branch,
            "tag": "target-tag",
            "dirty": dirty,
        })
    }
}

#[test]
#[serial_test::serial]
fn selected_workspace_receipt_identity_survives_caller_and_scheduler_changes() -> Result<()> {
    let fixture = Fixture::new();
    let outside = fixture.root.path().join("not-a-repository");
    fs::create_dir(&outside)?;
    let subdirectory = fixture.target.join("demo/src");
    let expected = fixture.expected_context(Some("target"), false);

    for caller in [&fixture.target, &subdirectory, &fixture.caller, &outside] {
        for flags in [&[][..], &["--parallel"][..]] {
            for actual in fixture.publish_and_resume(&fixture.target, caller, flags, None)? {
                assert_eq!(
                    actual,
                    expected,
                    "caller={}, flags={flags:?}",
                    caller.display()
                );
            }
        }
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn selected_workspace_receipt_supports_detached_dirty_and_linked_worktrees() -> Result<()> {
    let fixture = Fixture::new();
    git(&fixture.target, &["checkout", "--detach", "-q"]);
    write_file(
        &fixture.target.join("demo/src/lib.rs"),
        "pub fn changed() {}\n",
    );
    for actual in
        fixture.publish_and_resume(&fixture.target, &fixture.caller, &["--allow-dirty"], None)?
    {
        assert_eq!(actual, fixture.expected_context(None, true));
    }

    let linked = fixture.root.path().join("linked-worktree");
    git(
        &fixture.target,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().context("linked path UTF-8")?,
            "HEAD",
        ],
    );
    for actual in fixture.publish_and_resume(&linked, &fixture.caller, &["--parallel"], None)? {
        assert_eq!(actual, fixture.expected_context(None, false));
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn unavailable_target_git_evidence_never_borrows_the_caller_repository() -> Result<()> {
    let fixture = Fixture::new();
    let missing_git = fixture.root.path().join("missing-git-executable");
    for actual in fixture.publish_and_resume(
        &fixture.target,
        &fixture.caller,
        &["--allow-dirty"],
        Some(&missing_git),
    )? {
        assert!(actual.is_null(), "failed override must not use real Git");
    }

    fs::remove_dir_all(fixture.target.join(".git"))?;
    let caller_git = fixture.caller.join(".git");
    let foreign_repository = [
        ("GIT_DIR", caller_git.as_path()),
        ("GIT_WORK_TREE", fixture.caller.as_path()),
    ];
    for actual in fixture.publish_and_resume_with_git_env(
        &fixture.target,
        &fixture.caller,
        &["--allow-dirty", "--parallel"],
        None,
        &foreign_repository,
    )? {
        assert!(actual.is_null(), "non-Git target must not use caller Git");
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn receipt_git_override_queries_the_selected_workspace() -> Result<()> {
    let fixture = Fixture::new();
    let program = fixture.git_proxy()?;
    for actual in fixture.publish_and_resume(
        &fixture.target,
        &fixture.caller,
        &["--parallel"],
        Some(&program),
    )? {
        assert_eq!(actual, fixture.expected_context(Some("target"), false));
    }
    let calls = fs::read_to_string(fixture.root.path().join("git-calls"))?;
    ensure!(!calls.is_empty(), "Git override must be invoked");
    let expected_root = fixture.target.canonicalize()?;
    for caller in calls.lines() {
        assert_eq!(Path::new(caller).canonicalize()?, expected_root);
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn selected_workspace_receipts_ignore_ambient_repository_settings() -> Result<()> {
    let fixture = Fixture::new();
    let program = fixture.git_proxy()?;
    let caller_git = fixture.caller.join(".git");
    let foreign_repository = [
        ("GIT_DIR", caller_git.as_path()),
        ("GIT_WORK_TREE", fixture.caller.as_path()),
    ];
    let foreign_storage = [("GIT_COMMON_DIR", caller_git.as_path())];
    for git_env in [&foreign_repository[..], &foreign_storage[..]] {
        for git_override in [None, Some(program.as_path())] {
            for flags in [&[][..], &["--parallel"][..]] {
                for actual in fixture.publish_and_resume_with_git_env(
                    &fixture.target,
                    &fixture.caller,
                    flags,
                    git_override,
                    git_env,
                )? {
                    assert_eq!(actual, fixture.expected_context(Some("target"), false));
                }
            }
        }
    }
    Ok(())
}

#[test]
#[serial_test::serial]
fn dirty_selected_workspace_cannot_borrow_clean_repository_or_index() -> Result<()> {
    let fixture = Fixture::new();
    let program = fixture.git_proxy()?;
    fs::remove_file(fixture.caller.join("untracked"))?;
    let clean_index = fixture.root.path().join("alternate-clean-index");
    fs::copy(fixture.target.join(".git/index"), &clean_index)?;
    let source = fixture.target.join("demo/src/lib.rs");
    let original = fs::read(&source)?;
    write_file(&source, "pub fn staged_change() {}\n");
    git(&fixture.target, &["add", "demo/src/lib.rs"]);
    fs::write(&source, original)?;
    assert_eq!(git(&fixture.caller, &["status", "--porcelain"]), "");
    assert_eq!(
        git(&fixture.target, &["status", "--porcelain"]),
        "MM demo/src/lib.rs"
    );

    let caller_git = fixture.caller.join(".git");
    let caller_index = caller_git.join("index");
    let foreign_repository = [
        ("GIT_DIR", caller_git.as_path()),
        ("GIT_WORK_TREE", fixture.caller.as_path()),
        ("GIT_COMMON_DIR", caller_git.as_path()),
        ("GIT_INDEX_FILE", caller_index.as_path()),
    ];
    let alternate_index = [("GIT_INDEX_FILE", clean_index.as_path())];
    for git_env in [&foreign_repository[..], &alternate_index[..]] {
        for git_override in [None, Some(program.as_path())] {
            for actual in fixture.publish_and_resume_with_git_env(
                &fixture.target,
                &fixture.caller,
                &["--allow-dirty", "--parallel"],
                git_override,
                git_env,
            )? {
                assert_eq!(actual, fixture.expected_context(Some("target"), true));
            }
            for flags in [&[][..], &["--parallel"][..]] {
                fixture.assert_dirty_target_rejected(flags, git_override, git_env)?;
            }
        }
    }
    Ok(())
}
