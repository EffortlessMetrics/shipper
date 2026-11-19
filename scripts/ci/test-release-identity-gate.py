"""Exercise production release auth evidence and tag-only identity steps offline.

The workflow run block and step environment are consumed directly. Git is real;
the GitHub API and cargo metadata have bounded fixtures. The cargo shim only
dispatches the actual release-identity binary or serves fixture metadata, and
rejects every other command. This cannot publish or dispatch a workflow.
Metadata includes the approved package set without dependency edges; these cases
exercise authentication/source binding, not dependency-graph correctness.
"""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


PACKAGES = [
    "shipper", "shipper-cargo-failure", "shipper-cli", "shipper-config",
    "shipper-core", "shipper-duration", "shipper-encrypt",
    "shipper-output-sanitizer", "shipper-registry", "shipper-retry",
    "shipper-sparse-index", "shipper-types", "shipper-webhook",
]
TOKEN = "fixture-read-token-not-a-credential"
CONSUMERS = {
    "gate": ("release-identity-gate", "Validate approved source identity", "target/policy/release-identity.json"),
    "plan": ("publish-crates-io", "shipper plan", ".shipper/release-identity.json"),
}


def gate_step(workflow, consumer):
    job_name, step_name, _ = CONSUMERS[consumer]
    job = re.search(r"(?ms)^  " + re.escape(job_name) + r":\n(.*?)(?=^  [\w-]+:|\Z)", workflow)[1]
    step = job.split("      - name: " + step_name + "\n", 1)[1].split("\n      - ", 1)[0]
    before_run, body = step.split("        run: |\n", 1)
    script = "\n".join(line[10:] if line.startswith("          ") else line for line in body.splitlines())
    environment = {}
    if "        env:\n" in before_run:
        for line in before_run.split("        env:\n", 1)[1].splitlines():
            match = re.fullmatch(r"          ([A-Z_]+): (.+)", line)
            if match:
                key, value = match.groups()
                if value == "${{ secrets.GITHUB_TOKEN }}":
                    environment[key] = TOKEN
                else:
                    raise AssertionError(f"unrecognized gate environment input: {key}")
    for output in ["approved_sha", "approved_tree", "approved_version", "approval_record_ref", "approval_record_sha", "approved_registry", "approved_auth_posture"]:
        script = script.replace("${{ needs.release-identity-gate.outputs." + output + " }}", "$" + output.upper())
    assert "${{" not in script, "unrecognized workflow expression in exercised step"
    return script, environment


def write_executable(path, text):
    path.write_text(text, encoding="utf-8")
    path.chmod(0o755)


def checked(command, cwd, env):
    return subprocess.check_output(command, cwd=cwd, env=env, text=True, timeout=10).strip()


def run_case(workflow, xtask, *, consumer="gate", event="push", response="valid", strip_binding=False):
    script, step_environment = gate_step(workflow, consumer)
    if strip_binding:
        step_environment.pop("GH_TOKEN", None)
    with tempfile.TemporaryDirectory(prefix="shipper-release-gate-") as temp:
        root = Path(temp)
        shims = root / "bin"
        shims.mkdir()
        home = root / "home"
        home.mkdir()
        gh_config = root / "gh-empty"
        gh_config.mkdir()
        # Do not inherit Git overrides or ambient CLI credentials.
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("GIT_", "GH_", "GITHUB_"))}
        env.update({
            "HOME": str(home), "GH_CONFIG_DIR": str(gh_config),
            "GIT_AUTHOR_NAME": "Release fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
            "GIT_COMMITTER_NAME": "Release fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
            "GIT_AUTHOR_DATE": "2026-10-28T12:00:00Z", "GIT_COMMITTER_DATE": "2026-10-28T12:00:00Z",
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        })
        git = shutil.which("git")
        if not git:
            raise AssertionError("git is required for the source identity fixture")
        checked([git, "init", "-q"], root, env)
        checked([git, "config", "commit.gpgsign", "false"], root, env)
        checked([git, "config", "tag.gpgsign", "false"], root, env)
        (root / "CHANGELOG.md").write_text("## [0.5.0] - 2026-10-28\n", encoding="utf-8")
        (root / "RELEASE_NOTES_v0.5.0.md").write_text("Fixture release notes\n", encoding="utf-8")
        checked([git, "add", "CHANGELOG.md", "RELEASE_NOTES_v0.5.0.md"], root, env)
        checked([git, "commit", "-qm", "fixture source"], root, env)
        checked([git, "tag", "-a", "v0.5.0", "-m", "fixture tag"], root, env)
        sha = checked([git, "rev-parse", "HEAD"], root, env)
        tree = checked([git, "rev-parse", "HEAD^{tree}"], root, env)
        metadata = {"packages": [{"name": name, "version": "0.5.0", "dependencies": [], "publish": None} for name in PACKAGES]}
        (root / "metadata.json").write_text(json.dumps(metadata), encoding="utf-8")
        write_executable(shims / "gh", "#!" + sys.executable + "\n" + '''
import json, os, pathlib, sys
root = pathlib.Path(os.environ["FIXTURE_ROOT"])
with (root / "gh-calls.jsonl").open("a") as stream:
    stream.write(json.dumps(sys.argv[1:]) + "\\n")
assert sys.argv[1:] == ["api", "repos/EffortlessMetrics/shipper/git/ref/heads/main", "--jq", ".object.sha"]
if os.environ.get("GH_TOKEN") != "fixture-read-token-not-a-credential":
    print("fixture gh: missing explicit read token", file=sys.stderr)
    sys.exit(4)
response = os.environ["FIXTURE_RESPONSE"]
if response == "auth_failure":
    print("fixture gh: HTTP 401", file=sys.stderr)
    sys.exit(1)
if response == "api_missing":
    print("fixture gh: HTTP 404", file=sys.stderr)
    sys.exit(1)
print({"valid": os.environ["APPROVED_SHA"], "empty": "", "null": "null", "malformed": "not-a-sha", "drift": "d" * 40}[response])
''')
        write_executable(shims / "cargo", "#!" + sys.executable + "\n" + '''
import json, os, pathlib, sys
root = pathlib.Path(os.environ["FIXTURE_ROOT"])
with (root / "cargo-calls.jsonl").open("a") as stream:
    stream.write(json.dumps(sys.argv[1:]) + "\\n")
if sys.argv[1:] == ["metadata", "--format-version", "1", "--no-deps"]:
    print((root / "metadata.json").read_text())
elif sys.argv[1:3] == ["xtask", "release-identity"]:
    os.execv(os.environ["FIXTURE_XTASK"], [os.environ["FIXTURE_XTASK"]] + sys.argv[2:])
else:
    raise SystemExit("unexpected cargo command; publication is prohibited")
''')
        write_executable(shims / "shipper", "#!" + sys.executable + "\n" + '''
import json, os, pathlib, sys
root = pathlib.Path(os.environ["FIXTURE_ROOT"])
with (root / "shipper-calls.jsonl").open("a") as stream:
    stream.write(json.dumps(sys.argv[1:]) + "\\n")
assert sys.argv[1:] == ["plan", "--registry", "crates-io", "--state-dir", ".shipper", "--format", "json"]
assert (root / ".shipper/release-identity.json").is_file()
print('{"fixture_plan": true}')
''')
        env.update({
            "PATH": str(shims) + os.pathsep + os.environ["PATH"],
            "FIXTURE_ROOT": str(root), "FIXTURE_XTASK": str(xtask),
            "FIXTURE_RESPONSE": response, "GITHUB_EVENT_NAME": event,
            "GITHUB_REPOSITORY": "EffortlessMetrics/shipper", "GITHUB_REF_NAME": "v0.5.0",
            "GITHUB_ACTIONS": "true", "GITHUB_OUTPUT": str(root / "outputs"),
            "APPROVED_SHA": sha, "APPROVED_TREE": tree, "APPROVED_VERSION": "0.5.0",
            "APPROVAL_RECORD_REF": "fixture#477", "APPROVAL_RECORD_SHA": "fixture-record",
            "APPROVED_REGISTRY": "crates-io", "APPROVED_AUTH_POSTURE": "trusted_publishing",
        })
        env.update(step_environment)
        result = subprocess.run(["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", script], cwd=root, env=env, text=True, capture_output=True, timeout=10)
        identity_path = root / CONSUMERS[consumer][2]
        identity = json.loads(identity_path.read_text()) if identity_path.exists() else None
        outputs = (root / "outputs").read_text() if (root / "outputs").exists() else ""
        calls = {}
        for name in ["gh", "cargo", "shipper"]:
            path = root / (name + "-calls.jsonl")
            calls[name] = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        return result, identity, outputs, calls, sha


def test(workflow, xtask):
    for consumer in CONSUMERS:
        result, identity, outputs, calls, sha = run_case(workflow, xtask, consumer=consumer)
        assert result.returncode == 0, (consumer, result.stderr)
        assert identity["approved_sha"] == sha and identity["tag"] == "v0.5.0"
        assert (f"approved_sha={sha}\n" in outputs) == (consumer == "gate")
        assert len(calls["shipper"]) == (1 if consumer == "plan" else 0)
        assert len(calls["gh"]) == 1
        assert "--main-sha" in calls["cargo"][0] and "--tag-date" in calls["cargo"][0]
        for response, needle in [
            ("auth_failure", "HTTP 401"), ("api_missing", "HTTP 404"),
            ("empty", "40-character hexadecimal SHA"), ("null", "40-character hexadecimal SHA"),
            ("malformed", "40-character hexadecimal SHA"), ("drift", "release-authority main mismatch"),
        ]:
            result, identity, outputs, calls, _ = run_case(workflow, xtask, consumer=consumer, response=response)
            assert result.returncode != 0 and needle in result.stderr, (consumer, response, result.stderr)
            assert identity is None and not outputs and not calls["shipper"], (consumer, response)
            assert len(calls["gh"]) == 1, (consumer, response)
        result, identity, outputs, calls, _ = run_case(workflow, xtask, consumer=consumer, strip_binding=True)
        assert result.returncode != 0 and "missing explicit read token" in result.stderr
        assert identity is None and not outputs and not calls["cargo"] and not calls["shipper"]
    result, identity, outputs, calls, _ = run_case(workflow, xtask, event="workflow_dispatch", strip_binding=True)
    assert result.returncode == 0 and identity["tag"] is None, result.stderr
    assert not calls["gh"] and "--main-sha" not in calls["cargo"][0]
    print("Passed 17 actual-step cases: both authenticated tag reads, twelve failed reads, two removed bindings, non-tag dispatch.")


def test_auth_evidence(workflow):
    jobs = ("publish-crates-io", "release-rehearse", "release-resume")
    expressions = {
        "SHIPPER_RELEASE_AUTH_ACTION_OUTCOME": "${{ steps.auth.outcome }}",
        "SHIPPER_RELEASE_AUTH_TOKEN_MINTED": "${{ steps.auth.outputs.token != '' }}",
        "SHIPPER_RELEASE_AUTH_FALLBACK_CONFIGURED": "${{ secrets.CARGO_REGISTRY_TOKEN != '' }}",
        "SHIPPER_RELEASE_AUTH_RUN_ID": "${{ github.run_id }}",
        "SHIPPER_RELEASE_AUTH_RUN_ATTEMPT": "${{ github.run_attempt }}",
        "SHIPPER_RELEASE_AUTH_EVENT_COMMIT": "${{ github.sha }}",
        "APPROVED_SHA": "${{ needs.release-identity-gate.outputs.approved_sha }}",
        "APPROVED_TREE": "${{ needs.release-identity-gate.outputs.approved_tree }}",
    }
    sentinel = "dummy-sensitive-fixture-value-never-emitted"
    count = 0
    with tempfile.TemporaryDirectory(prefix="shipper-release-auth-") as temp:
        root = Path(temp)
        env = {"PATH": os.environ["PATH"], "HOME": temp, "GIT_CONFIG_NOSYSTEM": "1",
               "GIT_CONFIG_GLOBAL": os.devnull, "LC_ALL": "C"}
        checked(["git", "init", "--quiet"], root, env)
        for version in ("event", "approved"):
            (root / "version").write_text(version)
            checked(["git", "add", "version"], root, env)
            checked(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                     "commit", "-qm", version], root, env)
        event = checked(["git", "rev-parse", "HEAD^"], root, env)
        sha = checked(["git", "rev-parse", "HEAD"], root, env)
        tree = checked(["git", "rev-parse", "HEAD^{tree}"], root, env)
        event_tree = checked(["git", "rev-parse", "HEAD^^{tree}"], root, env)
        checked(["git", "checkout", "--quiet", "--detach", sha], root, env)
        assert event != sha and event_tree != tree

        def run(script, cwd, inputs):
            state = cwd / ".shipper"
            state.mkdir(exist_ok=True)
            (state / "state.json").write_text("unrelated-state")
            evidence = state / "auth-evidence.json"
            evidence.write_text('{"run_id":"predecessor"}')
            result = subprocess.run(["bash", "--noprofile", "--norc", "-c", script], cwd=cwd,
                                    env={**env, "CARGO_REGISTRY_TOKEN": sentinel, **inputs},
                                    text=True, capture_output=True, timeout=10)
            assert sentinel not in result.stdout + result.stderr
            assert (state / "state.json").read_text() == "unrelated-state"
            assert {path.name for path in state.iterdir()} <= {"state.json", "auth-evidence.json"}
            text = evidence.read_text() if evidence.exists() else None
            assert text is None or sentinel not in text
            return result, json.loads(text) if text is not None else None

        for name in jobs:
            job = re.search(r"(?ms)^  " + re.escape(name) + r":\n(.*?)(?=^  [\w-]+:|\Z)", workflow)[1]
            mint = job.index("      - name: Mint crates.io token (Trusted Publishing)")
            emitter = job.index("      - name: Record release auth evidence")
            assert job.index("cargo xtask release-identity") < mint < emitter, name
            step = job[emitter:].split("\n      - ", 1)[0]
            before, body = step.split("        run: |\n", 1)
            assert "        if: always()" in before, name
            assert dict(re.findall(r"^          ([A-Z_]+): (.+)$", before, re.M)) == expressions, name
            if name == "release-resume":
                assert "steps.auth-evidence.outcome != 'success' && '!.shipper/auth-evidence.json'" in job
            script = "\n".join(line[10:] for line in body.splitlines() if line.startswith("          "))
            common = {"APPROVED_SHA": sha, "APPROVED_TREE": tree,
                      "SHIPPER_RELEASE_AUTH_EVENT_COMMIT": event,
                      "SHIPPER_RELEASE_AUTH_RUN_ID": "99", "SHIPPER_RELEASE_AUTH_RUN_ATTEMPT": "2"}
            for selected, outcome, minted, fallback in [
                ("trusted_publishing", "success", "true", "true"),
                ("fallback_secret", "failure", "false", "true"),
                ("missing", "", "", ""),
            ]:
                inputs = {**common, "SHIPPER_RELEASE_AUTH_ACTION_OUTCOME": outcome,
                          "SHIPPER_RELEASE_AUTH_TOKEN_MINTED": minted,
                          "SHIPPER_RELEASE_AUTH_FALLBACK_CONFIGURED": fallback}
                result, receipt = run(script, root, inputs)
                assert result.returncode == 0, (name, selected, result.stderr)
                assert (receipt["job"], receipt["commit"], receipt["tree"], receipt["workflow_event_commit"],
                        receipt["run_id"], receipt["run_attempt"], receipt["selected_token_source"]) == (
                            name, sha, tree, event, "99", "2", selected)
                assert receipt["schema_version"] == "shipper.release_auth_evidence.v1"
                assert receipt["auth_action"]["outcome"] == outcome
                assert receipt["auth_action"]["token_minted"] == (minted == "true")
                assert receipt["fallback"] == {"configured": fallback == "true", "used": selected == "fallback_secret"}
                count += 1
            for field, wrong in [("APPROVED_SHA", event), ("APPROVED_TREE", event_tree)]:
                for value in (None, wrong):
                    inputs = dict(common, SHIPPER_RELEASE_AUTH_ACTION_OUTCOME="skipped")
                    if value is None:
                        del inputs[field]
                    else:
                        inputs[field] = value
                    result, receipt = run(script, root, inputs)
                    assert result.returncode != 0 and "does not match approval" in result.stdout, (name, field, value)
                    assert receipt is None, (name, field, value)
                    count += 1

        # Exercise a missing actual HEAD outside the detached checkout.
        with tempfile.TemporaryDirectory(prefix="shipper-release-auth-no-git-") as empty:
            result, receipt = run(script, Path(empty), common)
            assert result.returncode != 0 and receipt is None
            count += 1
    print(f"Passed {count} actual auth-emitter cases for checkout attribution, fallback and stale receipts.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--xtask", type=Path)
    mode.add_argument("--auth-only", action="store_true")
    parser.add_argument("--workflow", type=Path, default=Path(".github/workflows/release.yml"))
    args = parser.parse_args()
    workflow = args.workflow.read_text(encoding="utf-8")
    test_auth_evidence(workflow)
    if args.xtask is not None:
        test(workflow, args.xtask.resolve(strict=True))


if __name__ == "__main__":
    main()
