//! Source-owned taxonomy and drift checks for opt-in CI trigger labels.
//!
//! The policy file is authoritative. `check` is offline and validates both the
//! manifest and the workflow contract. `check-live` performs read-only GitHub
//! comparisons. `sync --apply` is the only mutating path; it creates or updates
//! the three configured labels, never deletes labels, and verifies the result.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use crate::workflow_checks::{
    job_level_if_expression, uncommented_workflow_text, workflow_job_blocks,
};

const MANIFEST: &str = "policy/ci-trigger-labels.toml";
const EXPECTED_SCHEMA: &str = "1.0";
const EXPECTED_POLICY: &str = "ci-trigger-labels";
const EXPECTED_REPOSITORY: &str = "EffortlessMetrics/shipper-swarm";

#[derive(Debug, Deserialize)]
struct LabelManifest {
    schema_version: String,
    policy: String,
    repository: String,
    owner: String,
    status: String,
    #[serde(default)]
    label: Vec<LabelSpec>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LabelSpec {
    name: String,
    color: String,
    description: String,
    owner: String,
    maintainer_controlled: bool,
    advisory: bool,
    broad_ci: bool,
    #[serde(default)]
    binding: Vec<WorkflowBinding>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct WorkflowBinding {
    workflow: String,
    job: String,
    activation: ActivationMode,
    accepted_pr_actions: Vec<String>,
    same_repository: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ActivationMode {
    LabelEvent,
    NextCodeEvent,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LiveLabel {
    name: String,
    color: String,
    description: Option<String>,
}

#[derive(Debug)]
enum SyncAction {
    Create(LabelSpec),
    Update(LabelSpec),
}

trait LabelApi {
    fn get(&mut self, repository: &str, name: &str) -> Result<Option<LiveLabel>>;
    fn create(&mut self, repository: &str, spec: &LabelSpec) -> Result<()>;
    fn update(&mut self, repository: &str, spec: &LabelSpec) -> Result<()>;
}

struct GhLabelApi;

impl LabelApi for GhLabelApi {
    fn get(&mut self, repository: &str, name: &str) -> Result<Option<LiveLabel>> {
        let endpoint = format!("repos/{repository}/labels/{name}");
        let output = Command::new("gh")
            .args(["api", endpoint.as_str()])
            .output()
            .with_context(|| format!("running gh api for label {name}"))?;
        if output.status.success() {
            let label: LiveLabel = serde_json::from_slice(&output.stdout)
                .with_context(|| format!("parsing live GitHub label {name}"))?;
            return Ok(Some(label));
        }

        let rendered = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if rendered.contains("HTTP 404") || rendered.contains("\"status\":\"404\"") {
            return Ok(None);
        }
        bail!("gh api failed while reading label {name}: {rendered}")
    }

    fn create(&mut self, repository: &str, spec: &LabelSpec) -> Result<()> {
        let endpoint = format!("repos/{repository}/labels");
        run_gh_mutation(
            &[
                "api",
                "--method",
                "POST",
                endpoint.as_str(),
                "-f",
                &format!("name={}", spec.name),
                "-f",
                &format!("color={}", spec.color),
                "-f",
                &format!("description={}", spec.description),
            ],
            &format!("creating label {}", spec.name),
        )
    }

    fn update(&mut self, repository: &str, spec: &LabelSpec) -> Result<()> {
        let endpoint = format!("repos/{repository}/labels/{}", spec.name);
        run_gh_mutation(
            &[
                "api",
                "--method",
                "PATCH",
                endpoint.as_str(),
                "-f",
                &format!("new_name={}", spec.name),
                "-f",
                &format!("color={}", spec.color),
                "-f",
                &format!("description={}", spec.description),
            ],
            &format!("updating label {}", spec.name),
        )
    }
}

fn run_gh_mutation(args: &[&str], operation: &str) -> Result<()> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .with_context(|| format!("running gh api while {operation}"))?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "gh api failed while {operation}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(crate) fn check() -> Result<()> {
    let root = workspace_root()?;
    let manifest = load_and_validate(&root)?;
    validate_workflow_contract(&root, &manifest)?;
    println!(
        "cargo xtask ci-labels check: repository={} labels={} status=ok",
        manifest.repository,
        manifest.label.len()
    );
    Ok(())
}

pub(crate) fn check_live(repository: &str) -> Result<()> {
    let root = workspace_root()?;
    let manifest = load_and_validate(&root)?;
    require_repository(&manifest, repository)?;
    validate_workflow_contract(&root, &manifest)?;
    let mut api = GhLabelApi;
    check_live_with(&manifest, &mut api)
}

pub(crate) fn sync(repository: &str, apply: bool) -> Result<()> {
    if !apply {
        bail!("live label synchronization requires the explicit --apply flag");
    }
    let root = workspace_root()?;
    let manifest = load_and_validate(&root)?;
    require_repository(&manifest, repository)?;
    validate_workflow_contract(&root, &manifest)?;
    let mut api = GhLabelApi;
    sync_with(&manifest, &mut api, apply)
}

fn load_and_validate(root: &Path) -> Result<LabelManifest> {
    let path = root.join(MANIFEST);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading CI trigger label manifest {}", path.display()))?;
    let manifest: LabelManifest = toml::from_str(&text)
        .with_context(|| format!("parsing CI trigger label manifest {}", path.display()))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn require_repository(manifest: &LabelManifest, requested: &str) -> Result<()> {
    if requested != manifest.repository {
        bail!(
            "requested repository {requested} does not match source-owned repository {}",
            manifest.repository
        );
    }
    Ok(())
}

fn validate_manifest(manifest: &LabelManifest) -> Result<()> {
    if manifest.schema_version != EXPECTED_SCHEMA {
        bail!(
            "unsupported CI trigger label schema {}; expected {EXPECTED_SCHEMA}",
            manifest.schema_version
        );
    }
    if manifest.policy != EXPECTED_POLICY {
        bail!(
            "unexpected CI trigger label policy {}; expected {EXPECTED_POLICY}",
            manifest.policy
        );
    }
    if manifest.repository != EXPECTED_REPOSITORY {
        bail!(
            "CI trigger labels must target {EXPECTED_REPOSITORY}, not {}",
            manifest.repository
        );
    }
    if manifest.owner != "release/ci" {
        bail!("CI trigger label manifest owner must be release/ci");
    }
    if manifest.status != "active" {
        bail!("CI trigger label manifest status must be active");
    }

    let expected_names = BTreeSet::from(["coverage", "full-ci", "mutation"]);
    let mut names = BTreeSet::new();
    for spec in &manifest.label {
        if !names.insert(spec.name.as_str()) {
            bail!("duplicate CI trigger label: {}", spec.name);
        }
        if !spec
            .name
            .chars()
            .all(|character| character.is_ascii_lowercase() || character == '-')
        {
            bail!("CI trigger label names must be lowercase ASCII/kebab-case");
        }
        if spec.color.len() != 6
            || !spec
                .color
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            bail!(
                "label {} color must be exactly six hexadecimal digits",
                spec.name
            );
        }
        if spec.description.trim().len() < 20 || spec.description.len() > 100 {
            bail!(
                "label {} description must contain 20..=100 characters",
                spec.name
            );
        }
        if spec.owner != manifest.owner {
            bail!(
                "label {} owner {} must match manifest owner {}",
                spec.name,
                spec.owner,
                manifest.owner
            );
        }
        if !spec.maintainer_controlled || !spec.advisory || spec.broad_ci {
            bail!(
                "label {} must remain maintainer-controlled, advisory, and outside broad ci.yml",
                spec.name
            );
        }
        if !expected_names.contains(spec.name.as_str()) {
            bail!("unsupported CI trigger label: {}", spec.name);
        }
        if spec.binding.is_empty() {
            bail!("label {} must bind at least one workflow job", spec.name);
        }
        let mut binding_owners = BTreeSet::new();
        for binding in &spec.binding {
            if !binding_owners.insert((binding.workflow.as_str(), binding.job.as_str())) {
                bail!(
                    "label {} duplicates workflow/job binding {}/{}",
                    spec.name,
                    binding.workflow,
                    binding.job
                );
            }
            if !binding.workflow.starts_with(".github/workflows/")
                || !binding.workflow.ends_with(".yml")
                || binding.job.trim().is_empty()
            {
                bail!(
                    "label {} binding must name a .github/workflows/*.yml file and job",
                    spec.name
                );
            }
            if !binding.same_repository {
                bail!(
                    "label {} binding must remain same-repository-only",
                    spec.name
                );
            }
            let actual_actions = binding
                .accepted_pr_actions
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            if actual_actions.len() != binding.accepted_pr_actions.len() {
                bail!("label {} binding contains duplicate PR actions", spec.name);
            }
            let allowed_actions = ["labeled", "opened", "reopened", "synchronize"]
                .into_iter()
                .collect::<BTreeSet<_>>();
            if actual_actions.is_empty() || !actual_actions.is_subset(&allowed_actions) {
                bail!("label {} binding has unsupported PR actions", spec.name);
            }
            match binding.activation {
                ActivationMode::LabelEvent if !actual_actions.contains("labeled") => {
                    bail!(
                        "label {} label-event binding must accept labeled",
                        spec.name
                    );
                }
                ActivationMode::NextCodeEvent if actual_actions.contains("labeled") => {
                    bail!(
                        "label {} next-code-event binding must not accept labeled",
                        spec.name
                    );
                }
                _ => {}
            }
        }
    }
    let actual = names.into_iter().collect::<Vec<_>>();
    let required = expected_names.into_iter().collect::<Vec<_>>();
    if actual != required {
        bail!("CI trigger labels must be exactly {required:?}, not {actual:?}");
    }
    Ok(())
}

fn validate_workflow_contract(root: &Path, manifest: &LabelManifest) -> Result<()> {
    let mut declared_predicates: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for spec in &manifest.label {
        for binding in &spec.binding {
            let source = read(root, &binding.workflow)?;
            validate_workflow_binding(spec, binding, &source)?;
            declared_predicates
                .entry((&binding.workflow, &binding.job))
                .or_default()
                .insert(&spec.name);
        }
    }
    for ((workflow, job_name), declared) in declared_predicates {
        let source = uncommented_workflow_text(&read(root, workflow)?);
        let jobs = workflow_job_blocks(&source);
        let job = jobs
            .iter()
            .find_map(|(name, block)| (name == job_name).then_some(block))
            .with_context(|| format!("{workflow} is missing configured job {job_name}"))?;
        validate_declared_label_predicates(workflow, job_name, job, &declared)?;
    }
    Ok(())
}

fn validate_declared_label_predicates(
    workflow: &str,
    job_name: &str,
    job: &str,
    declared: &BTreeSet<&str>,
) -> Result<()> {
    let expression = job_level_if_expression(job)
        .with_context(|| format!("{workflow} job {job_name} is missing a job-level if guard"))?;
    let observed = label_predicates(&expression)?;
    if observed != *declared {
        bail!(
            "{workflow} job {job_name} label predicates {observed:?} do not match declared bindings {declared:?}"
        );
    }
    Ok(())
}

fn label_predicates(job: &str) -> Result<BTreeSet<&str>> {
    let pattern = Regex::new(
        r#"contains\(\s*github\.event\.pull_request\.labels\.\*\.name\s*,\s*['\"]([^'\"]+)['\"]\s*\)"#,
    )
    .context("compiling workflow label predicate parser")?;
    Ok(pattern
        .captures_iter(job)
        .filter_map(|capture| capture.get(1).map(|value| value.as_str()))
        .collect())
}

fn validate_workflow_binding(
    spec: &LabelSpec,
    binding: &WorkflowBinding,
    source: &str,
) -> Result<()> {
    let uncommented = uncommented_workflow_text(source);
    let jobs = workflow_job_blocks(&uncommented);
    let job = jobs
        .iter()
        .find_map(|(name, block)| (name == &binding.job).then_some(block))
        .with_context(|| {
            format!(
                "{} is missing configured job {} for label {}",
                binding.workflow, binding.job, spec.name
            )
        })?;
    let expression = job_level_if_expression(job).with_context(|| {
        format!(
            "{} job {} is missing a job-level if guard",
            binding.workflow, binding.job
        )
    })?;
    require_contains(
        &format!("{} job {}", binding.workflow, binding.job),
        &expression,
        &format!(
            "contains(github.event.pull_request.labels.*.name, '{}')",
            spec.name
        ),
    )?;
    require_contains(
        &format!("{} job {}", binding.workflow, binding.job),
        &expression,
        "github.event.pull_request.head.repo.full_name == github.repository",
    )?;
    let workflow_actions = pull_request_actions(&uncommented)?;
    let expected_actions = binding
        .accepted_pr_actions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if workflow_actions != expected_actions {
        bail!(
            "{} pull_request actions {:?} do not match label {} binding {:?}",
            binding.workflow,
            workflow_actions,
            spec.name,
            expected_actions
        );
    }
    Ok(())
}

fn pull_request_actions(source: &str) -> Result<BTreeSet<&str>> {
    let mut in_on = false;
    let mut on_indent = 0usize;
    let mut in_pull_request = false;
    let mut pull_request_indent = 0usize;
    for line in source.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        if !in_on && indent == 0 && trimmed == "on:" {
            in_on = true;
            on_indent = indent;
            continue;
        }
        if in_on && !trimmed.is_empty() && indent <= on_indent {
            break;
        }
        if in_on && !in_pull_request && indent == on_indent + 2 && trimmed == "pull_request:" {
            in_pull_request = true;
            pull_request_indent = indent;
            continue;
        }
        if in_pull_request && !trimmed.is_empty() && indent <= pull_request_indent {
            break;
        }
        if in_pull_request && let Some(value) = trimmed.strip_prefix("types:") {
            let actions = value
                .trim()
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
                .context("pull_request types must use an inline action list")?
                .split(',')
                .map(str::trim)
                .filter(|action| !action.is_empty())
                .collect::<BTreeSet<_>>();
            return Ok(actions);
        }
    }
    bail!("workflow is missing pull_request types")
}

fn read(root: &Path, relative: &str) -> Result<String> {
    let path = root.join(relative);
    fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn require_contains(owner: &str, text: &str, required: &str) -> Result<()> {
    if !text.contains(required) {
        bail!("{owner} is missing the configured CI label contract: {required}");
    }
    Ok(())
}

fn check_live_with<A: LabelApi>(manifest: &LabelManifest, api: &mut A) -> Result<()> {
    let drift = collect_drift(manifest, api)?;
    if !drift.is_empty() {
        bail!("live CI trigger label drift: {}", drift.join("; "));
    }
    println!(
        "cargo xtask ci-labels check-live: repository={} labels={} drift=0",
        manifest.repository,
        manifest.label.len()
    );
    Ok(())
}

fn collect_drift<A: LabelApi>(manifest: &LabelManifest, api: &mut A) -> Result<Vec<String>> {
    let mut drift = Vec::new();
    for spec in &manifest.label {
        match api.get(&manifest.repository, &spec.name)? {
            None => drift.push(format!("{} missing", spec.name)),
            Some(live) => {
                if !live_matches(spec, &live) {
                    drift.push(format!(
                        "{} expected color={} description={:?}; live color={} description={:?}",
                        spec.name, spec.color, spec.description, live.color, live.description
                    ));
                }
            }
        }
    }
    Ok(drift)
}

fn live_matches(spec: &LabelSpec, live: &LiveLabel) -> bool {
    live.name == spec.name
        && live.color.eq_ignore_ascii_case(&spec.color)
        && live.description.as_deref() == Some(spec.description.as_str())
}

fn sync_with<A: LabelApi>(manifest: &LabelManifest, api: &mut A, apply: bool) -> Result<()> {
    if !apply {
        bail!("live label synchronization requires the explicit --apply flag");
    }
    // Resolve every read before the first mutation. A missing permission,
    // unavailable API, or malformed live label must not leave a partially
    // synchronized taxonomy merely because it appeared after an earlier
    // configured label in the manifest.
    let actions = plan_sync(manifest, api)?;
    if actions.is_empty() {
        println!("plan: no CI trigger label changes");
    } else {
        for action in &actions {
            println!("{}", describe_action(action));
        }
    }
    let mut created = 0usize;
    let mut updated = 0usize;
    for action in actions {
        match action {
            SyncAction::Create(spec) => {
                api.create(&manifest.repository, &spec)?;
                created += 1;
            }
            SyncAction::Update(spec) => {
                api.update(&manifest.repository, &spec)?;
                updated += 1;
            }
        }
    }
    let drift = collect_drift(manifest, api)?;
    if !drift.is_empty() {
        bail!(
            "live CI trigger labels still drift after sync: {}",
            drift.join("; ")
        );
    }
    println!(
        "cargo xtask ci-labels sync --apply: repository={} created={} updated={} drift=0",
        manifest.repository, created, updated
    );
    Ok(())
}

fn describe_action(action: &SyncAction) -> String {
    let (operation, spec) = match action {
        SyncAction::Create(spec) => ("create", spec),
        SyncAction::Update(spec) => ("update", spec),
    };
    format!(
        "plan: {operation} label={} color={} description={:?}",
        spec.name, spec.color, spec.description
    )
}

fn plan_sync<A: LabelApi>(manifest: &LabelManifest, api: &mut A) -> Result<Vec<SyncAction>> {
    let mut actions = Vec::new();
    for spec in &manifest.label {
        match api.get(&manifest.repository, &spec.name)? {
            None => actions.push(SyncAction::Create(spec.clone())),
            Some(live) if !live_matches(spec, &live) => {
                actions.push(SyncAction::Update(spec.clone()));
            }
            Some(_) => {}
        }
    }
    Ok(actions)
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set; run via cargo xtask")?;
    PathBuf::from(manifest_dir)
        .parent()
        .map(Path::to_path_buf)
        .context("xtask manifest directory has no workspace parent")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeApi {
        labels: BTreeMap<String, LiveLabel>,
        mutations: Vec<String>,
        fail_on_get: Option<String>,
        fail_on_mutation: Option<String>,
        ignore_mutation: Option<String>,
    }

    impl LabelApi for FakeApi {
        fn get(&mut self, _repository: &str, name: &str) -> Result<Option<LiveLabel>> {
            if self.fail_on_get.as_deref() == Some(name) {
                bail!("injected read failure for {name}");
            }
            Ok(self.labels.get(name).cloned())
        }

        fn create(&mut self, _repository: &str, spec: &LabelSpec) -> Result<()> {
            if self.fail_on_mutation.as_deref() == Some(spec.name.as_str()) {
                bail!("injected create failure for {}", spec.name);
            }
            self.mutations.push(format!("create:{}", spec.name));
            if self.ignore_mutation.as_deref() != Some(spec.name.as_str()) {
                self.labels.insert(spec.name.clone(), live(spec));
            }
            Ok(())
        }

        fn update(&mut self, _repository: &str, spec: &LabelSpec) -> Result<()> {
            if self.fail_on_mutation.as_deref() == Some(spec.name.as_str()) {
                bail!("injected update failure for {}", spec.name);
            }
            self.mutations.push(format!("update:{}", spec.name));
            if self.ignore_mutation.as_deref() != Some(spec.name.as_str()) {
                self.labels.insert(spec.name.clone(), live(spec));
            }
            Ok(())
        }
    }

    fn valid_manifest() -> LabelManifest {
        LabelManifest {
            schema_version: EXPECTED_SCHEMA.to_string(),
            policy: EXPECTED_POLICY.to_string(),
            repository: EXPECTED_REPOSITORY.to_string(),
            owner: "release/ci".to_string(),
            status: "active".to_string(),
            label: vec![
                spec("coverage", "0e8a16", vec![coverage_binding()]),
                spec(
                    "full-ci",
                    "5319e7",
                    vec![coverage_binding(), mutation_binding()],
                ),
                spec("mutation", "b60205", vec![mutation_binding()]),
            ],
        }
    }

    fn spec(name: &str, color: &str, binding: Vec<WorkflowBinding>) -> LabelSpec {
        LabelSpec {
            name: name.to_string(),
            color: color.to_string(),
            description: format!("Opt in to the advisory {name} CI evidence lane"),
            owner: "release/ci".to_string(),
            maintainer_controlled: true,
            advisory: true,
            broad_ci: false,
            binding,
        }
    }

    fn coverage_binding() -> WorkflowBinding {
        WorkflowBinding {
            workflow: ".github/workflows/coverage.yml".to_string(),
            job: "coverage".to_string(),
            activation: ActivationMode::NextCodeEvent,
            accepted_pr_actions: vec![
                "opened".to_string(),
                "reopened".to_string(),
                "synchronize".to_string(),
            ],
            same_repository: true,
        }
    }

    fn mutation_binding() -> WorkflowBinding {
        WorkflowBinding {
            workflow: ".github/workflows/mutation.yml".to_string(),
            job: "mutants-pr".to_string(),
            activation: ActivationMode::LabelEvent,
            accepted_pr_actions: vec![
                "opened".to_string(),
                "reopened".to_string(),
                "synchronize".to_string(),
                "labeled".to_string(),
            ],
            same_repository: true,
        }
    }

    fn live(spec: &LabelSpec) -> LiveLabel {
        LiveLabel {
            name: spec.name.clone(),
            color: spec.color.clone(),
            description: Some(spec.description.clone()),
        }
    }

    #[test]
    fn manifest_accepts_only_the_bounded_taxonomy() {
        validate_manifest(&valid_manifest()).expect("valid bounded taxonomy");

        let mut extra = valid_manifest();
        extra
            .label
            .push(spec("expensive", "123abc", vec![mutation_binding()]));
        let error = validate_manifest(&extra).expect_err("extra label must fail");
        assert!(error.to_string().contains("unsupported"), "{error:#}");

        let mut expanded = valid_manifest();
        expanded.label[0].binding[0]
            .accepted_pr_actions
            .push("labeled".to_string());
        let error = validate_manifest(&expanded).expect_err("expanded action set must fail");
        assert!(error.to_string().contains("next-code-event"), "{error:#}");

        let mut owner_drift = valid_manifest();
        owner_drift.label[0].owner = "tests".to_string();
        let error = validate_manifest(&owner_drift).expect_err("owner disagreement must fail");
        assert!(error.to_string().contains("must match"), "{error:#}");

        let error = require_repository(&valid_manifest(), "someone/else")
            .expect_err("repository mismatch must fail before API access");
        assert!(error.to_string().contains("does not match"), "{error:#}");
    }

    #[test]
    fn workflow_binding_rejects_comment_spoof_wrong_job_actions_and_missing_guard() {
        let coverage = spec("coverage", "0e8a16", vec![coverage_binding()]);
        let binding = &coverage.binding[0];
        let valid = "on:\n  pull_request:\n    types: [opened, reopened, synchronize]\njobs:\n  coverage:\n    if: github.event.pull_request.head.repo.full_name == github.repository && contains(github.event.pull_request.labels.*.name, 'coverage')\n    runs-on: ubuntu-latest\n";
        validate_workflow_binding(&coverage, binding, valid).expect("valid binding");

        let comment_spoof = valid.replace(
            "contains(github.event.pull_request.labels.*.name, 'coverage')",
            "true # contains(github.event.pull_request.labels.*.name, 'coverage')",
        );
        let error = validate_workflow_binding(&coverage, binding, &comment_spoof)
            .expect_err("comment-only predicate must fail");
        assert!(error.to_string().contains("missing"), "{error:#}");

        let wrong_job = valid.replace(
            "jobs:\n  coverage:",
            "jobs:\n  decoy:\n    if: github.event.pull_request.head.repo.full_name == github.repository && contains(github.event.pull_request.labels.*.name, 'coverage')\n    runs-on: ubuntu-latest\n  coverage:",
        ).replace(
            "  coverage:\n    if: github.event.pull_request.head.repo.full_name == github.repository && contains(github.event.pull_request.labels.*.name, 'coverage')",
            "  coverage:\n    if: true",
        );
        let error = validate_workflow_binding(&coverage, binding, &wrong_job)
            .expect_err("predicate in another job must fail");
        assert!(error.to_string().contains("job coverage"), "{error:#}");

        let missing_guard = valid.replace(
            "github.event.pull_request.head.repo.full_name == github.repository && ",
            "",
        );
        let error = validate_workflow_binding(&coverage, binding, &missing_guard)
            .expect_err("missing same-repository guard must fail");
        assert!(error.to_string().contains("missing"), "{error:#}");

        let labeled = valid.replace(
            "types: [opened, reopened, synchronize]",
            "types: [opened, reopened, synchronize, labeled]",
        );
        let error = validate_workflow_binding(&coverage, binding, &labeled)
            .expect_err("coverage labeled trigger must fail");
        assert!(error.to_string().contains("actions"), "{error:#}");

        let run_step_decoy = valid.replace(
            "if: github.event.pull_request.head.repo.full_name == github.repository && contains(github.event.pull_request.labels.*.name, 'coverage')\n    runs-on:",
            "if: github.event.pull_request.head.repo.full_name == github.repository\n    steps:\n      - run: echo \"contains(github.event.pull_request.labels.*.name, 'coverage')\"\n    runs-on:",
        );
        let error = validate_workflow_binding(&coverage, binding, &run_step_decoy)
            .expect_err("run-step label text must not satisfy the job guard");
        assert!(error.to_string().contains("job coverage"), "{error:#}");

        let decoy_mapping = valid.replacen(
            "on:\n  pull_request:\n    types: [opened, reopened, synchronize]",
            "decoy:\n  pull_request:\n    types: [opened, reopened, synchronize]\non:\n  pull_request:\n    types: [opened, reopened, synchronize, labeled]",
            1,
        );
        let error = validate_workflow_binding(&coverage, binding, &decoy_mapping)
            .expect_err("decoy mapping must not supply pull_request actions");
        assert!(error.to_string().contains("actions"), "{error:#}");

        let undeclared = valid.replace(
            "contains(github.event.pull_request.labels.*.name, 'coverage')",
            "contains(github.event.pull_request.labels.*.name, 'coverage') || contains(github.event.pull_request.labels.*.name, 'surprise')",
        );
        let uncommented = uncommented_workflow_text(&undeclared);
        let job = workflow_job_blocks(&uncommented)
            .into_iter()
            .find_map(|(name, block)| (name == "coverage").then_some(block))
            .expect("coverage job");
        let error = validate_declared_label_predicates(
            ".github/workflows/coverage.yml",
            "coverage",
            &job,
            &BTreeSet::from(["coverage"]),
        )
        .expect_err("undeclared label predicate must fail");
        assert!(error.to_string().contains("surprise"), "{error:#}");
    }

    #[test]
    fn live_check_reports_missing_and_metadata_drift_without_mutation() {
        let manifest = valid_manifest();
        let mut api = FakeApi::default();
        api.labels
            .insert("coverage".to_string(), live(&manifest.label[0]));
        let mut drifted = live(&manifest.label[1]);
        drifted.color = "ffffff".to_string();
        api.labels.insert("full-ci".to_string(), drifted);
        let mut nullable = live(&manifest.label[2]);
        nullable.description = None;
        api.labels.insert("mutation".to_string(), nullable);

        let error = check_live_with(&manifest, &mut api).expect_err("drift must fail");
        let rendered = error.to_string();
        assert!(rendered.contains("full-ci expected"), "{rendered}");
        assert!(rendered.contains("mutation expected"), "{rendered}");
        assert!(api.mutations.is_empty());
    }

    #[test]
    fn sync_requires_apply_and_does_not_mutate_without_it() {
        let manifest = valid_manifest();
        let mut api = FakeApi::default();
        let error = sync_with(&manifest, &mut api, false).expect_err("apply is required");
        assert!(error.to_string().contains("--apply"), "{error:#}");
        assert!(api.labels.is_empty());
        assert!(api.mutations.is_empty());
    }

    #[test]
    fn sync_creates_missing_updates_drift_and_preserves_exact_labels() {
        let manifest = valid_manifest();
        let mut api = FakeApi::default();
        api.labels
            .insert("coverage".to_string(), live(&manifest.label[0]));
        let mut drifted = live(&manifest.label[1]);
        drifted.description = Some("stale description".to_string());
        api.labels.insert("full-ci".to_string(), drifted);
        api.labels.insert(
            "unrelated".to_string(),
            LiveLabel {
                name: "unrelated".to_string(),
                color: "ffffff".to_string(),
                description: Some("must remain untouched".to_string()),
            },
        );

        sync_with(&manifest, &mut api, true).expect("bounded sync");
        assert_eq!(api.mutations, vec!["update:full-ci", "create:mutation"]);
        assert!(api.labels.contains_key("unrelated"));
        check_live_with(&manifest, &mut api).expect("sync must converge");
        api.mutations.clear();
        sync_with(&manifest, &mut api, true).expect("idempotent rerun");
        assert!(api.mutations.is_empty());
    }

    #[test]
    fn mid_sync_failure_preserves_unrelated_labels_and_idempotent_rerun_converges() {
        let manifest = valid_manifest();
        let mut api = FakeApi {
            fail_on_mutation: Some("mutation".to_string()),
            ..FakeApi::default()
        };
        api.labels
            .insert("coverage".to_string(), live(&manifest.label[0]));
        let mut drifted = live(&manifest.label[1]);
        drifted.color = "ffffff".to_string();
        api.labels.insert("full-ci".to_string(), drifted);
        api.labels.insert(
            "unrelated".to_string(),
            LiveLabel {
                name: "unrelated".to_string(),
                color: "abcdef".to_string(),
                description: Some("preserved across partial apply".to_string()),
            },
        );

        let error = sync_with(&manifest, &mut api, true).expect_err("injected partial failure");
        assert!(error.to_string().contains("mutation"), "{error:#}");
        assert_eq!(api.mutations, vec!["update:full-ci"]);
        assert!(api.labels.contains_key("unrelated"));

        api.fail_on_mutation = None;
        sync_with(&manifest, &mut api, true).expect("rerun converges after partial apply");
        assert_eq!(api.mutations, vec!["update:full-ci", "create:mutation"]);
        assert!(api.labels.contains_key("unrelated"));
        check_live_with(&manifest, &mut api).expect("converged taxonomy");
    }

    #[test]
    fn sync_fails_when_post_write_readback_still_drifts() {
        let manifest = valid_manifest();
        let mut api = FakeApi {
            ignore_mutation: Some("coverage".to_string()),
            ..FakeApi::default()
        };
        let error = sync_with(&manifest, &mut api, true)
            .expect_err("successful API response without convergence must fail");
        assert!(error.to_string().contains("still drift"), "{error:#}");
    }

    #[test]
    fn sync_preflights_every_read_before_the_first_mutation() {
        let manifest = valid_manifest();
        let mut api = FakeApi {
            fail_on_get: Some("mutation".to_string()),
            ..FakeApi::default()
        };

        let error = sync_with(&manifest, &mut api, true).expect_err("late read must fail");
        assert!(error.to_string().contains("mutation"), "{error:#}");
        assert!(api.mutations.is_empty());
        assert!(api.labels.is_empty());
    }
}
