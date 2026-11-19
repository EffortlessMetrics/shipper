"""Test the release selector offline; opt in to source-bound cross-run rehearsal.

The gh stub serves paginated API fixtures; Bash and jq execute the workflow's
production selector. Action inputs are checked against download-artifact v8's
single-ID contract (contents extracted directly into path, no name wrapper).
"""

import argparse
import hashlib
import io
import runpy
import stat
import zipfile
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib


def step(job, name):
    return job.split("      - name: " + name + "\n", 1)[1].split("\n      - ", 1)[0]


def selector(workflow):
    job = workflow.split("  release-resume:\n", 1)[1]
    selected = step(job, "Select prior .shipper/ state artifact")
    body = selected.split("        run: |\n", 1)[1]
    script = "\n".join(line[10:] if line.startswith("          ") else line for line in body.splitlines())
    assert "          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}" in selected
    assert "          ARTIFACT_RUN_ID: ${{ inputs.artifact_run_id }}" in selected
    download = step(job, "Download prior .shipper/ state")
    assert "uses: actions/download-artifact@v8" in download
    inputs = dict(re.findall(r"^          ([\w-]+): (.+)$", download, re.M))
    assert inputs == {
        "artifact-ids": "${{ steps.prior-state.outputs.artifact_id }}",
        "path": ".shipper/", "run-id": "${{ github.event.inputs.artifact_run_id }}",
        "github-token": "${{ secrets.GITHUB_TOKEN }}",
    }, inputs
    return script


def artifact(**changes):
    return {"id": 123, "name": "shipper-state-final", "expired": False,
            "workflow_run": {"id": 17}, **changes}


def run(script, pages, run_id="17", api_failure=False):
    with tempfile.TemporaryDirectory(prefix="shipper-artifact-selector-") as temp:
        root = Path(temp)
        (root / "response").write_text(pages if isinstance(pages, str) else json.dumps(pages))
        gh = root / "gh"
        gh.write_text("#!" + sys.executable + "\n" + '''
import os, pathlib, sys
root = pathlib.Path(os.environ["FIXTURE_ROOT"])
(root / "called").write_text("called")
assert sys.argv[1:] == ["api", "--paginate", "--slurp", "repos/EffortlessMetrics/shipper/actions/runs/17/artifacts?per_page=100"]
assert os.environ.get("GH_TOKEN") == "fixture-actions-read"
if os.environ["API_FAILURE"] == "true":
    print("fixture HTTP 403", file=sys.stderr)
    sys.exit(1)
print((root / "response").read_text())
''')
        gh.chmod(0o755)
        env = {key: value for key, value in os.environ.items() if not key.startswith(("GH_", "GITHUB_"))}
        env.update({"PATH": str(root) + os.pathsep + env["PATH"], "FIXTURE_ROOT": str(root),
                    "GH_TOKEN": "fixture-actions-read", "ARTIFACT_RUN_ID": run_id,
                    "GITHUB_REPOSITORY": "EffortlessMetrics/shipper",
                    "GITHUB_OUTPUT": str(root / "outputs"), "GITHUB_STEP_SUMMARY": str(root / "summary"),
                    "API_FAILURE": str(api_failure).lower()})
        result = subprocess.run(["bash", "-c", script], cwd=root, env=env, capture_output=True, text=True, timeout=5)
        output = (root / "outputs").read_text() if (root / "outputs").exists() else ""
        summary = (root / "summary").read_text() if (root / "summary").exists() else ""
        return result, output, summary, (root / "called").exists()


def test(workflow):
    script = selector(workflow)
    # Include an unrelated first page: --paginate --slurp must feed every page.
    valid = [{"artifacts": [artifact(id=99, name="unrelated")]}, {"artifacts": [artifact()]}]
    result, output, summary, called = run(script, valid)
    assert result.returncode == 0 and output == "artifact_id=123\n", result.stderr
    assert called and "run `17`, artifact `123`" in summary
    failures = [
        ([{"artifacts": []}], "missing or ambiguous evidence"),
        ([{"artifacts": [artifact()]}, {"artifacts": [artifact(id=124)]}], "missing or ambiguous evidence"),
        ([{"artifacts": [artifact(expired=True)]}], "expired or has unknown expiry"),
        ([{"artifacts": [artifact(expired=None)]}], "expired or has unknown expiry"),
        ([{"artifacts": [artifact(workflow_run={"id": 18})]}], "does not belong to the requested run"),
        ([{"artifacts": [artifact(id="123")]}], "invalid ID"),
        ([{"artifacts": [artifact(id=1.5)]}], "invalid ID"),
        ("not-json", "parse error"),
    ]
    for pages, expected in failures:
        result, output, summary, _ = run(script, pages)
        assert result.returncode != 0 and expected in result.stderr, result.stderr
        assert not output and not summary
    for invalid in ["", "-1", "17; echo bad", "017"]:
        result, output, summary, called = run(script, valid, run_id=invalid)
        assert result.returncode != 0 and "positive artifact_run_id" in result.stdout
        assert not output and not summary and not called
    result, output, summary, _ = run(script, valid, api_failure=True)
    assert result.returncode != 0 and "HTTP 403" in result.stderr
    assert not output and not summary
    print("Passed 14 actual-selector cases: paginated valid predecessor and 13 refusal controls.")


# This is a test harness, not a second release admission authority. The live
# opt-in consumes the production selector above and the landed Rust phase test.
REPOSITORY = "EffortlessMetrics/shipper-swarm"
HARNESS_WORKFLOW = ".github/workflows/cross-run-recovery-rehearsal.yml"
PROVENANCE = "cross-run-harness.json"
REGISTRY = "127.0.0.1:39197"
PHASES = {"seed": 1, "retry": 2, "finish": 3}
MAX_ARCHIVE = 16 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def positive_id(value):
    require(isinstance(value, str) and re.fullmatch(r"[1-9][0-9]*", value) is not None,
            "run ID must be a canonical positive decimal")
    return value


def checked(args, *, env=None, stdout=None):
    result = subprocess.run(args, env=env, stdout=stdout or subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=60)
    # Do not copy arbitrary API output or environment values into diagnostics.
    require(result.returncode == 0, f"{args[0]} command failed (exit {result.returncode})")
    return result.stdout


def api(path):
    return json.loads(checked(["gh", "api", "--method", "GET", path]))


def toolchain_pin():
    pin = tomllib.loads(Path("rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]["channel"]
    require(isinstance(pin, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", pin) is not None,
            "rehearsal requires an exact stable checkout toolchain pin")
    return pin


def select_toolchain():
    pin = toolchain_pin()
    with Path(os.environ["GITHUB_ENV"]).open("a", encoding="utf-8") as output:
        output.write(f"RUSTUP_TOOLCHAIN={pin}\n")


def verify_toolchain():
    pin = toolchain_pin()
    require(os.environ.get("RUSTUP_TOOLCHAIN") == pin, "selected toolchain differs from checkout pin")
    rustc = checked(["rustc", "+" + pin, "-Vv"]).decode().strip()
    cargo = checked(["cargo", "+" + pin, "-V"]).decode().strip()
    require(f"release: {pin}" in rustc.splitlines(), "rustc release differs from checkout pin")
    require(cargo.startswith(f"cargo {pin} "), "cargo release differs from checkout pin")
    source = os.environ.get("GITHUB_SHA", "")
    require(re.fullmatch(r"[0-9a-f]{40}", source) is not None, "invalid source SHA")
    phase = os.environ.get("PHASE")
    require(phase in PHASES, "unknown rehearsal phase")
    receipt = {"format": "rehearsal-toolchain-v1", "source": source,
               "run_id": positive_id(os.environ.get("GITHUB_RUN_ID", "")),
               "run_attempt": positive_id(os.environ.get("GITHUB_RUN_ATTEMPT", "")),
               "phase": phase, "pin": pin, "rustc_verbose": rustc, "cargo_version": cargo}
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    (Path(os.environ["RUNNER_TEMP"]) / "shipper-toolchain.json").write_text(encoded, encoding="utf-8")
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as summary:
        summary.write("### Actual rehearsal toolchain (not release authority)\n\n```json\n" + encoded + "```\n")


def toolchain_tests():
    from unittest.mock import Mock, patch
    with tempfile.TemporaryDirectory(prefix="shipper-toolchain-test-") as directory:
        root = Path(directory)
        env = {"RUSTUP_TOOLCHAIN": "1.95.0", "GITHUB_SHA": "a" * 40,
               "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "PHASE": "seed",
               "RUNNER_TEMP": str(root), "GITHUB_STEP_SUMMARY": str(root / "summary"),
               "GITHUB_ENV": str(root / "env")}
        identity = root / "shipper-toolchain.json"
        # Execute the real helpers with bounded command responses, never a registry.
        with patch.dict(os.environ, env), patch.object(Path, "read_text", return_value='[toolchain]\nchannel = "1.95.0"\n'):
            select_toolchain()
            commands = Mock(side_effect=[b"rustc 1.95.0 (fixture)\nrelease: 1.95.0\nhost: fixture", b"cargo 1.95.0 (fixture)"])
            with patch.dict(globals(), checked=commands):
                verify_toolchain()
                assert commands.call_args_list[0].args[0] == ["rustc", "+1.95.0", "-Vv"]
                assert commands.call_args_list[1].args[0] == ["cargo", "+1.95.0", "-V"]
        receipt = json.loads(identity.read_text())
        assert receipt["pin"] == "1.95.0" and receipt["phase"] == "seed" and receipt["source"] == "a" * 40
        assert (root / "env").read_text() == "RUSTUP_TOOLCHAIN=1.95.0\n"
        before = identity.read_bytes()
        controls = 0
        for pin, selected, rustc, cargo in [
            ("stable", "stable", "release: 1.95.0", "cargo 1.95.0 (fixture)"),
            ("1.95.0\nINJECT=x", "1.95.0", "release: 1.95.0", "cargo 1.95.0 (fixture)"),
            ("1.95.0", "1.100.0", "release: 1.95.0", "cargo 1.95.0 (fixture)"),
            ("1.95.0", "1.95.0", "release: 1.100.0", "cargo 1.95.0 (fixture)"),
            ("1.95.0", "1.95.0", "release: 1.95.0", "cargo 1.100.0 (fixture)"),
        ]:
            with patch.dict(os.environ, {**env, "RUSTUP_TOOLCHAIN": selected}), \
                 patch.object(Path, "read_text", return_value="[toolchain]\nchannel = " + json.dumps(pin) + "\n"), \
                 patch.dict(globals(), checked=Mock(side_effect=[rustc.encode(), cargo.encode()])):
                try:
                    verify_toolchain()
                except ValueError:
                    controls += 1
                else:
                    raise AssertionError("incorrect compiler identity accepted")
            assert identity.read_bytes() == before
        assert controls == 5
    print("Passed toolchain selection/identity positive and 5 refusal controls.")


def live_context(phase, predecessor):
    require(os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch", "manual dispatch only")
    require(os.environ.get("GITHUB_REPOSITORY") == REPOSITORY, "swarm rehearsal only")
    require(phase in PHASES, "unknown rehearsal phase")
    run_id = positive_id(os.environ.get("GITHUB_RUN_ID", ""))
    attempt = positive_id(os.environ.get("GITHUB_RUN_ATTEMPT", ""))
    if phase == "seed":
        require(predecessor == "", "seed must not specify a predecessor")
    else:
        positive_id(predecessor)
        require(predecessor != run_id, "self-predecessor is forbidden")
    root = Path(os.environ["RUNNER_TEMP"]) / "shipper-cross-run-recovery"
    require(Path(os.environ["SHIPPER_LIVE_REHEARSAL_ROOT"]).resolve() == root.resolve(),
            "unexpected fixture root")
    require(os.environ.get("SHIPPER_LIVE_REHEARSAL_REGISTRY_ADDR") == REGISTRY,
            "fixture registry must remain loopback")
    source = os.environ.get("GITHUB_SHA", "")
    require(re.fullmatch(r"[0-9a-f]{40}", source) is not None, "invalid source SHA")
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("GIT_")}
    tree = checked(["git", "rev-parse", "HEAD^{tree}"], env=env).decode().strip()
    proof = runpy.run_path("scripts/release/verify_binary.py")
    lock_digest = proof["source_identity"](source, tree)
    return {"format": "test-harness-v1", "repository": REPOSITORY, "source": source,
            "tree": tree, "cargo_lock_sha256": lock_digest, "phase": phase,
            "run_id": run_id, "run_attempt": attempt, "predecessor_run_id": predecessor,
            "root": str(root.resolve()), "registry": REGISTRY, "package_version": "0.1.0"}


def validate_run(run, context):
    require(str(run.get("id")) == context["predecessor_run_id"], "wrong predecessor run")
    require(run.get("event") == "workflow_dispatch" and run.get("path") == HARNESS_WORKFLOW,
            "predecessor must be this manual rehearsal workflow")
    require(run.get("status") == "completed" and run.get("conclusion") == "success",
            "predecessor harness did not complete successfully")
    require(run.get("head_sha") == context["source"], "predecessor source differs")
    for field in ("repository", "head_repository"):
        require(run.get(field, {}).get("full_name") == REPOSITORY, "foreign predecessor repository")
    positive_id(str(run.get("run_attempt", "")))


def validate_artifact(metadata, artifact_id, context):
    require(type(metadata.get("id")) is int and str(metadata["id"]) == artifact_id,
            "wrong artifact ID")
    require(metadata.get("name") == "shipper-state-final" and metadata.get("expired") is False,
            "unavailable final-state artifact")
    origin = metadata.get("workflow_run", {})
    require(str(origin.get("id")) == context["predecessor_run_id"]
            and origin.get("head_sha") == context["source"], "artifact source/run differs")
    require(type(metadata.get("size_in_bytes")) is int
            and 0 < metadata["size_in_bytes"] <= MAX_ARCHIVE, "artifact exceeds fixture budget")
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", metadata.get("digest", "")) is not None,
            "artifact SHA-256 unavailable")


def unpack_checked(archive, expected_digest):
    require(0 < len(archive) <= MAX_ARCHIVE and "sha256:" + sha256(archive) == expected_digest,
            "downloaded archive digest/size mismatch")
    files = {}
    seen = set()
    with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
        require(len(zipped.infolist()) <= 128, "too many fixture archive entries")
        require(sum(item.file_size for item in zipped.infolist()) <= MAX_ARCHIVE,
                "expanded fixture exceeds budget")
        for item in zipped.infolist():
            name = item.filename
            parts = name.rstrip("/").split("/")
            require(name and not name.startswith("/") and ":" not in name and "\\" not in name
                    and all(part not in ("", ".", "..") for part in parts), "unsafe archive path")
            canonical = "/".join(parts)
            require(canonical not in seen, "duplicate archive member")
            seen.add(canonical)
            mode = item.external_attr >> 16
            require(stat.S_IFMT(mode) in (0, stat.S_IFREG, stat.S_IFDIR), "nonregular archive member")
            if item.is_dir():
                continue
            require(name not in files, "duplicate archive member")
            files[name] = zipped.read(item)
    require(not any(parent in files for name in files
                    for parent in (str(p) for p in Path(name).parents if str(p) != ".")),
            "archive file/directory collision")
    return files


def checkpoint(files, phase):
    count = PHASES[phase]
    state = json.loads(files["state.json"])
    events = [json.loads(line) for line in files["events.jsonl"].splitlines() if line.strip()]
    packages = state["packages"]
    require(set(packages) == {f"crate-{name}@0.1.0" for name in "abc"}, "fixture package/version mismatch")
    require(isinstance(state.get("plan_id"), str) and state["plan_id"], "missing fixture plan ID")
    log = files["fake-cargo.log"].decode().splitlines()
    uploads = {name: sum("publish" in line and f"crate-{name}" in line for line in log) for name in "abc"}
    require(uploads == {"a": 1, "b": 1, "c": count}, "unexpected cumulative upload counts")
    require(type(packages["crate-c@0.1.0"]["attempts"]) is int
            and packages["crate-c@0.1.0"]["attempts"] == count, "attempt ceiling/history mismatch")
    states = {name: packages[f"crate-{name}@0.1.0"]["state"]["state"] for name in "abc"}
    require(states["a"] == states["b"] == "published", "completed packages regressed")
    require((states["c"] == "published") == (phase == "finish"), "wrong phase completion")
    require(("receipt.json" in files) == (phase == "finish"), "wrong phase receipt presence")
    starts = sum(event.get("event_type", {}).get("type") == "execution_started" for event in events)
    require(starts == count, "execution segments lost or duplicated")
    return {"plan_id": state["plan_id"], "uploads": uploads, "execution_segments": starts,
            "event_bytes": len(files["events.jsonl"]), "event_sha256": sha256(files["events.jsonl"])}


def validate_predecessor(files, run, context):
    receipt = json.loads(files[PROVENANCE])
    for key in ("format", "repository", "source", "tree", "cargo_lock_sha256", "root", "registry", "package_version"):
        require(receipt.get(key) == context[key], f"checkpoint {key} differs")
    expected_phase = {"retry": "seed", "finish": "retry"}[context["phase"]]
    require(receipt.get("phase") == expected_phase, "not the immediate predecessor phase")
    require(receipt.get("run_id") == context["predecessor_run_id"]
            and receipt.get("run_attempt") == str(run["run_attempt"]), "checkpoint run/attempt differs")
    if expected_phase == "seed":
        require(receipt.get("predecessor_run_id") == "", "seed has an unexpected predecessor")
    else:
        predecessor = positive_id(receipt.get("predecessor_run_id", ""))
        require(predecessor not in (receipt["run_id"], context["run_id"]), "invalid prior chain")
    expected = {name: sha256(data) for name, data in files.items() if name != PROVENANCE}
    require(receipt.get("files") == expected, "checkpoint file set/digests differ")
    evidence = checkpoint(files, expected_phase)
    require(receipt.get("checkpoint") == evidence, "checkpoint summary disagrees with actual files")
    return evidence


def context_path():
    return Path(os.environ["RUNNER_TEMP"]) / "shipper-cross-run-context.json"


def prepare(context, workflow):
    root = Path(context["root"])
    require(not root.exists(), "rehearsal requires a fresh fixture root")
    if context["phase"] != "seed":
        base = f"repos/{REPOSITORY}/actions"
        run = api(f"{base}/runs/{context['predecessor_run_id']}")
        validate_run(run, context)
        with tempfile.TemporaryDirectory(prefix="shipper-cross-run-") as temp:
            temp = Path(temp)
            env = dict(os.environ, ARTIFACT_RUN_ID=context["predecessor_run_id"],
                       GITHUB_OUTPUT=str(temp / "outputs"), GITHUB_STEP_SUMMARY=str(temp / "summary"))
            # Execute the same production selector, not a second selection algorithm.
            checked(["bash", "-c", selector(workflow)], env=env)
            match = re.fullmatch(r"artifact_id=([1-9][0-9]*)\n", (temp / "outputs").read_text())
            require(match is not None, "selector did not return one artifact ID")
            artifact_id = match.group(1)
            metadata = api(f"{base}/artifacts/{artifact_id}")
            validate_artifact(metadata, artifact_id, context)
            archive = temp / "artifact.zip"
            with archive.open("wb") as output:
                checked(["gh", "api", "--method", "GET", f"{base}/artifacts/{artifact_id}/zip"], stdout=output)
            require(archive.stat().st_size <= MAX_ARCHIVE, "download exceeds fixture budget")
            files = unpack_checked(archive.read_bytes(), metadata["digest"])
            current_run = api(f"{base}/runs/{context['predecessor_run_id']}")
            validate_run(current_run, context)
            require(current_run["run_attempt"] == run["run_attempt"], "predecessor was rerun during transfer")
            context["predecessor_checkpoint"] = validate_predecessor(files, run, context)
            context["predecessor_artifact"] = {"id": artifact_id, "digest": metadata["digest"],
                                                "run_attempt": str(run["run_attempt"])}
            # No destination exists until every metadata, digest and phase check passes.
            state_dir = root / ".shipper"
            state_dir.mkdir(parents=True)
            for name, data in files.items():
                destination = state_dir / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(data)
    context_path().write_text(json.dumps(context, indent=2) + "\n")


def record(context):
    prepared = json.loads(context_path().read_text())
    require(all(prepared.get(key) == value for key, value in context.items()), "prepared context changed")
    state_dir = Path(context["root"]) / ".shipper"
    files = {}
    for path in state_dir.rglob("*"):
        require(not path.is_symlink(), "fixture contains a symlink")
        relative = path.relative_to(state_dir).as_posix()
        if path.is_file() and relative != PROVENANCE:
            require(path.stat().st_size <= MAX_ARCHIVE, "fixture file exceeds budget")
            files[relative] = path.read_bytes()
    require(len(files) <= 127 and sum(map(len, files.values())) <= MAX_ARCHIVE,
            "fixture checkpoint exceeds budget")
    evidence = checkpoint(files, context["phase"])
    if context["phase"] != "seed":
        previous = prepared["predecessor_checkpoint"]
        current = files["events.jsonl"]
        require(len(current) > previous["event_bytes"]
                and sha256(current[:previous["event_bytes"]]) == previous["event_sha256"],
                "continuation did not preserve the exact predecessor event prefix")
        require(evidence["plan_id"] == previous["plan_id"], "continuation changed the plan")
    prepared.update(checkpoint=evidence, files={name: sha256(data) for name, data in files.items()})
    (state_dir / PROVENANCE).write_text(json.dumps(prepared, indent=2, sort_keys=True) + "\n")
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as summary:
        summary.write("### Cross-run fixture evidence (not release authority)\n\n```json\n"
                      + json.dumps(prepared, indent=2, sort_keys=True) + "\n```\n")


def cross_run_tests():
    context = {"format": "test-harness-v1", "repository": REPOSITORY, "source": "a" * 40,
               "tree": "b" * 40, "cargo_lock_sha256": "c" * 64, "phase": "retry",
               "run_id": "18", "run_attempt": "1", "predecessor_run_id": "17",
               "root": "/tmp/fixture", "registry": REGISTRY, "package_version": "0.1.0"}
    run_data = {"id": 17, "run_attempt": 1, "event": "workflow_dispatch", "path": HARNESS_WORKFLOW,
                "status": "completed", "conclusion": "success", "head_sha": context["source"],
                "repository": {"full_name": REPOSITORY}, "head_repository": {"full_name": REPOSITORY}}
    files = {
        "state.json": json.dumps({"plan_id": "fixture-plan", "packages": {
            f"crate-{name}@0.1.0": {"attempts": 1, "state": {"state": "failed" if name == "c" else "published"}}
            for name in "abc"}}).encode(),
        "events.jsonl": b'{"event_type":{"type":"execution_started"}}\n',
        "fake-cargo.log": b'publish crate-a\npublish crate-b\npublish crate-c\n',
    }
    receipt = {**context, "run_id": "17", "phase": "seed", "predecessor_run_id": "",
               "checkpoint": checkpoint(files, "seed"), "files": {name: sha256(data) for name, data in files.items()}}
    files[PROVENANCE] = json.dumps(receipt).encode()
    validate_run(run_data, context)
    validate_predecessor(files, run_data, context)
    def rejected(call):
        try:
            call()
        except (ValueError, KeyError):
            return
        raise AssertionError("invalid cross-run fixture was accepted")
    controls = 0
    for key, value in [("id", 16), ("event", "pull_request"), ("path", "other.yml"),
                       ("status", "in_progress"), ("conclusion", "failure"), ("head_sha", "d" * 40),
                       ("repository", {"full_name": "other/repo"}), ("head_repository", {"full_name": "fork/repo"}),
                       ("run_attempt", 0)]:
        rejected(lambda: validate_run({**run_data, key: value}, context)); controls += 1
    for key, value in [("source", "d" * 40), ("tree", "d" * 40), ("run_id", "16"),
                       ("run_attempt", "2"), ("phase", "retry"), ("registry", "crates.io:443"),
                       ("package_version", "0.2.0"), ("root", "/other"), ("files", {})]:
        changed = {**files, PROVENANCE: json.dumps({**receipt, key: value}).encode()}
        rejected(lambda: validate_predecessor(changed, run_data, context)); controls += 1
    # The finish phase must reject a valid but stale seed, not just corrupt inputs.
    rejected(lambda: validate_predecessor(files, run_data, {**context, "phase": "finish"})); controls += 1
    rejected(lambda: validate_predecessor({**files, "events.jsonl": b'changed'}, run_data, context)); controls += 1
    for invalid in ["", "0", "-1", "017", "17; echo bad"]:
        rejected(lambda: positive_id(invalid)); controls += 1
    metadata = {"id": 123, "name": "shipper-state-final", "expired": False,
                "size_in_bytes": 100, "digest": "sha256:" + "f" * 64,
                "workflow_run": {"id": 17, "head_sha": context["source"]}}
    validate_artifact(metadata, "123", context)
    for key, value in [("id", 124), ("id", "123"), ("name", "shipper-state-preflight"),
                       ("expired", True), ("expired", None), ("size_in_bytes", 0),
                       ("size_in_bytes", MAX_ARCHIVE + 1), ("digest", ""),
                       ("workflow_run", {"id": 16, "head_sha": context["source"]}),
                       ("workflow_run", {"id": 17, "head_sha": "d" * 40})]:
        rejected(lambda: validate_artifact({**metadata, key: value}, "123", context)); controls += 1
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w") as zipped:
        for name, data in files.items(): zipped.writestr(name, data)
    raw = archive.getvalue()
    assert unpack_checked(raw, "sha256:" + sha256(raw)) == files
    rejected(lambda: unpack_checked(raw, "sha256:" + "0" * 64)); controls += 1
    for name in ["../state.json", "/state.json", "a\\state.json", "./state.json"]:
        bad = io.BytesIO()
        with zipfile.ZipFile(bad, "w") as zipped: zipped.writestr(name, b'bad')
        data = bad.getvalue()
        rejected(lambda: unpack_checked(data, "sha256:" + sha256(data))); controls += 1
    info = zipfile.ZipInfo("state.json"); info.external_attr = (stat.S_IFLNK | 0o777) << 16
    bad = io.BytesIO()
    with zipfile.ZipFile(bad, "w") as zipped: zipped.writestr(info, b'/outside')
    data = bad.getvalue()
    rejected(lambda: unpack_checked(data, "sha256:" + sha256(data))); controls += 1
    for entries in [[("a", b"x"), ("a/b", b"y")], [(f"f-{i}", b"x") for i in range(129)]]:
        bad = io.BytesIO()
        with zipfile.ZipFile(bad, "w") as zipped:
            for name, payload in entries: zipped.writestr(name, payload)
        data = bad.getvalue()
        rejected(lambda: unpack_checked(data, "sha256:" + sha256(data))); controls += 1
    assert checkpoint(files, "seed")["uploads"] == {"a": 1, "b": 1, "c": 1}
    print(f"Passed cross-run positive controls and {controls} refusing provenance/archive fixtures.")


def transport_tests(workflow):
    """Run prepare/record with a fake GitHub API, real Bash/jq, and real ZIP I/O.

    Synthetic checkpoint data is NOT the Rust CLI or a hosted-run receipt.
    The landed phase driver and three real manual runs remain separate proof.
    """
    import shutil
    from unittest.mock import patch

    with tempfile.TemporaryDirectory(prefix="shipper-cross-run-test-") as temp:
        directory = Path(temp)
        root = directory / "workspace"
        gh = directory / "gh"
        gh.write_text("#!" + sys.executable + "\n" + '''
import json, os, pathlib, sys
root = pathlib.Path(os.environ["API_FIXTURE"])
args = sys.argv[1:]
assert os.environ["GH_TOKEN"] == "test-actions-read"
assert args[0] == "api"
endpoint = args[-1]
base = "repos/EffortlessMetrics/shipper-swarm/actions/"
assert endpoint.startswith(base)
endpoint = endpoint[len(base):]
if args[1:3] == ["--paginate", "--slurp"]:
    assert endpoint == "runs/" + os.environ["ARTIFACT_RUN_ID"] + "/artifacts?per_page=100"
    metadata = json.loads((root / "metadata.json").read_text())
    print(json.dumps([{"artifacts": []}, {"artifacts": [metadata]}]))
else:
    assert args[1:3] == ["--method", "GET"]
    if endpoint.startswith("runs/"):
        sys.stdout.write((root / "run.json").read_text())
    elif endpoint == "artifacts/123":
        sys.stdout.write((root / "metadata.json").read_text())
    elif endpoint == "artifacts/123/zip":
        sys.stdout.buffer.write((root / "archive.zip").read_bytes())
    else:
        raise AssertionError(endpoint)
''')
        gh.chmod(0o755)
        context = {"format": "test-harness-v1", "repository": REPOSITORY, "source": "a" * 40,
                   "tree": "b" * 40, "cargo_lock_sha256": "c" * 64, "phase": "seed",
                   "run_id": "17", "run_attempt": "1", "predecessor_run_id": "",
                   "root": str(root), "registry": REGISTRY, "package_version": "0.1.0"}
        environment = {"RUNNER_TEMP": temp, "PATH": temp + os.pathsep + os.environ["PATH"],
                       "API_FIXTURE": temp, "GH_TOKEN": "test-actions-read",
                       "GITHUB_REPOSITORY": REPOSITORY, "GITHUB_STEP_SUMMARY": str(directory / "summary")}
        with patch.dict(os.environ, environment):
            for phase, run_id, predecessor in [("seed", "17", ""), ("retry", "18", "17"), ("finish", "19", "18")]:
                context = {**context, "phase": phase, "run_id": run_id, "predecessor_run_id": predecessor}
                if phase != "seed":
                    before = (directory / "archive.zip").read_bytes()
                    # A valid seed offered to finish must be refused before the root exists.
                    if phase == "retry":
                        try:
                            prepare({**context, "phase": "finish"}, workflow)
                        except ValueError as error:
                            assert "predecessor phase" in str(error)
                        else:
                            raise AssertionError("stale seed restored")
                        assert not root.exists() and (directory / "archive.zip").read_bytes() == before
                prepare(dict(context), workflow)
                state_dir = root / ".shipper"
                state_dir.mkdir(parents=True, exist_ok=True)
                count = PHASES[phase]
                previous_events = (state_dir / "events.jsonl").read_bytes() if phase != "seed" else b""
                state = {"plan_id": "fixture-plan", "packages": {
                    f"crate-{name}@0.1.0": {"attempts": count if name == "c" else 1,
                        "state": {"state": "published" if name != "c" or phase == "finish" else "failed"}}
                    for name in "abc"}}
                (state_dir / "state.json").write_text(json.dumps(state))
                (state_dir / "events.jsonl").write_bytes(previous_events + b'{"event_type":{"type":"execution_started"}}\n')
                (state_dir / "fake-cargo.log").write_text("publish crate-a\npublish crate-b\n" + "publish crate-c\n" * count)
                if phase == "finish":
                    (state_dir / "receipt.json").write_text("{}")
                record(context)
                receipt = json.loads((state_dir / PROVENANCE).read_text())
                assert receipt["checkpoint"]["uploads"] == {"a": 1, "b": 1, "c": count}
                if phase != "seed":
                    assert receipt["predecessor_artifact"]["id"] == "123"
                # Save precisely the directory contents as artifact upload would.
                with zipfile.ZipFile(directory / "archive.zip", "w") as archive:
                    for path in state_dir.iterdir():
                        archive.write(path, path.name)
                raw = (directory / "archive.zip").read_bytes()
                metadata = {"id": 123, "name": "shipper-state-final", "expired": False,
                            "size_in_bytes": len(raw), "digest": "sha256:" + sha256(raw),
                            "workflow_run": {"id": int(run_id), "head_sha": context["source"]}}
                (directory / "metadata.json").write_text(json.dumps(metadata))
                run_data = {"id": int(run_id), "run_attempt": 1, "event": "workflow_dispatch",
                            "path": HARNESS_WORKFLOW, "status": "completed", "conclusion": "success",
                            "head_sha": context["source"], "repository": {"full_name": REPOSITORY},
                            "head_repository": {"full_name": REPOSITORY}}
                (directory / "run.json").write_text(json.dumps(run_data))
                shutil.rmtree(root)
    print("Passed synthetic three-stage prepare/record transport and stale-seed no-restoration control.")


def workflow_contract(text):
    # Reuse the existing explicit-layout parser; actionlint owns YAML syntax.
    sections = runpy.run_path(str(Path(__file__).with_name("check-full-ci-hosted.py")))["sections"]
    document = sections(text, 0)
    require(set(document) == {"name", "on", "permissions", "jobs"}, "unexpected workflow override")
    require(set(sections(document["on"], 2)) == {"workflow_dispatch"}, "manual-only trigger required")
    require(document["permissions"].strip() == "contents: read", "global permissions expanded")
    jobs = sections(document["jobs"], 2)
    require(set(jobs) == {"rehearse"}, "one deliberate phase job required")
    job = sections(jobs["rehearse"], 4)
    require(set(job) == {"if", "runs-on", "timeout-minutes", "permissions", "env", "steps"},
            "unexpected job override")
    require(job["if"] == "github.repository == 'EffortlessMetrics/shipper-swarm' && github.event_name == 'workflow_dispatch'",
            "swarm/manual admission changed")
    require(job["runs-on"] == "ubuntu-24.04" and job["timeout-minutes"] == "30", "runtime route/budget changed")
    require(sections(job["permissions"], 6) == {"contents": "read", "actions": "read"}, "job authority expanded")
    require(sections(job["env"], 6) == {
        "PHASE": "${{ inputs.phase }}", "PREDECESSOR": "${{ inputs.predecessor_run_id }}",
        "SHIPPER_CROSS_RUN_PHASE": "${{ inputs.phase }}",
        "SHIPPER_LIVE_REHEARSAL_REGISTRY_ADDR": REGISTRY}, "fixture context changed")
    steps = [sections("        " + part, 8) for part in re.split(r"(?m)^      - ", job["steps"])[1:]]
    require(len(steps) == 10, "unexpected phase step count")
    require(steps[0] == {"uses": "actions/checkout@v7.0.1", "with":
                        "          ref: ${{ github.sha }}\n          persist-credentials: false"}, "checkout not pinned")
    for index, mode in [(1, "prepare"), (7, "record")]:
        step_data = steps[index]
        require(set(step_data) == {"name", "run", "env"},
                "phase proof skipped or waived")
        require(step_data["run"] == 'python3 scripts/ci/test-release-resume-artifacts.py --mode '
                + mode + ' --phase "$PHASE" --predecessor-run-id "$PREDECESSOR"', "phase proof command changed")
    # runner.temp exists at step evaluation, not in job-level env.
    fixture_env = {"SHIPPER_LIVE_REHEARSAL_ROOT": "${{ runner.temp }}/shipper-cross-run-recovery"}
    require(sections(steps[1]["env"], 10) == {**fixture_env, "GH_TOKEN": "${{ github.token }}"},
            "unexpected prepare environment")
    for index in (6, 7):
        require(sections(steps[index].get("env", ""), 10) == fixture_env,
                "phase step must retain the runner-local fixture root")
    require(steps[2] == {"name": "Select checkout toolchain", "run":
        "python3 scripts/ci/test-release-resume-artifacts.py --mode select-toolchain"}, "toolchain selection changed")
    require(steps[3] == {"name": "Install Rust", "uses": "dtolnay/rust-toolchain@master", "with":
        "          toolchain: ${{ env.RUSTUP_TOOLCHAIN }}"}, "toolchain install changed")
    require(steps[4] == {"name": "Verify actual toolchain", "run":
        "python3 scripts/ci/test-release-resume-artifacts.py --mode verify-toolchain"}, "toolchain verification changed")
    require(set(steps[5]) == {"name", "uses", "with"} and steps[5]["uses"] == "actions/cache@v6", "cache step changed")
    cache = sections(steps[5]["with"], 10)
    require(set(cache) == {"path", "key", "restore-keys"}
            and cache["path"].removeprefix("|").split() == ["~/.cargo/registry", "~/.cargo/git", "target"]
            and cache["key"] == "hosted-cross-run-${{ runner.os }}-${{ hashFiles('**/Cargo.lock') }}"
            and cache["restore-keys"] == "hosted-cross-run-${{ runner.os }}-", "cache isolation changed")
    require(set(steps[6]) == {"name", "run", "env"} and steps[6]["run"] ==
            'cargo "+$RUSTUP_TOOLCHAIN" test -p shipper-cli --test e2e_rehearse cross_run::hosted_recovery_phase --locked -- --ignored --exact --nocapture',
            "actual phase execution changed")
    require(set(steps[8]) == {"name", "if", "uses", "with"} and steps[8]["if"] == "always()"
            and steps[8]["uses"] == "actions/upload-artifact@v7", "checkpoint retention changed")
    require(sections(steps[8]["with"], 10) == {
        "name": "shipper-state-final", "path": "${{ runner.temp }}/shipper-cross-run-recovery/.shipper/",
        "include-hidden-files": "true", "if-no-files-found": "error", "retention-days": "30"}, "wrong checkpoint upload")

    require(steps[9] == {"name": "Upload actual toolchain identity", "if": "always()", "uses": "actions/upload-artifact@v7", "with":
        "          name: shipper-toolchain\n          path: ${{ runner.temp }}/shipper-toolchain.json\n          if-no-files-found: error\n          retention-days: 30"}, "toolchain retention changed")

def workflow_tests():
    text = Path(HARNESS_WORKFLOW).read_text()
    workflow_contract(text)
    mutations = [("--mode select-toolchain", "--mode verify-toolchain"),
                 ("--mode verify-toolchain", "--mode select-toolchain"),
                 ('cargo "+$RUSTUP_TOOLCHAIN" test', "cargo test"),
                 ("name: shipper-toolchain", "name: unrelated-toolchain"),
                 ("workflow_dispatch:\n", "pull_request:\n"),
                 ("github.repository == 'EffortlessMetrics/shipper-swarm'", "true"),
                 ("actions: read", "actions: write"), ("contents: read", "contents: write"),
                 ("ref: ${{ github.sha }}", "ref: main"),
                 ("persist-credentials: false", "persist-credentials: true"),
                 ("--mode prepare", "--mode record"),
                 ("      - name: Execute one controlled recovery phase\n", "      - name: Execute one controlled recovery phase\n        if: false\n"),
                 ("      - name: Verify and record continuation evidence\n", "      - name: Verify and record continuation evidence\n        continue-on-error: true\n"),
                 ("include-hidden-files: true", "include-hidden-files: false"),
                 ("if: always()", "if: success()"),
                 ("name: shipper-state-final", "name: unrelated"),
                 ("127.0.0.1:39197", "crates.io:443"),
                 ("timeout-minutes: 30", "timeout-minutes: 360"),
                 ("      PHASE: ${{ inputs.phase }}\n",
                  "      SHIPPER_LIVE_REHEARSAL_ROOT: ${{ runner.temp }}/shipper-cross-run-recovery\n      PHASE: ${{ inputs.phase }}\n"),
                 ("          SHIPPER_LIVE_REHEARSAL_ROOT: ${{ runner.temp }}/shipper-cross-run-recovery\n", "")]
    for old, new in mutations:
        assert old in text
        try:
            workflow_contract(text.replace(old, new, 1))
        except ValueError:
            pass
        else:
            raise AssertionError(f"workflow regression accepted: {old}")
    workflow_contract(text.replace("name: Cross-run recovery rehearsal", "name: Renamed display label"))
    print(f"Passed workflow positive controls and {len(mutations)} rejected workflow regressions.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workflow", type=Path, default=Path(".github/workflows/release.yml"))
    parser.add_argument("--mode", choices=("test", "prepare", "record", "select-toolchain", "verify-toolchain"), default="test")
    parser.add_argument("--phase", choices=tuple(PHASES))
    parser.add_argument("--predecessor-run-id", default="")
    args = parser.parse_args()
    if args.mode in ("select-toolchain", "verify-toolchain"):
        (select_toolchain if args.mode == "select-toolchain" else verify_toolchain)()
        return
    workflow = args.workflow.read_text(encoding="utf-8")
    if args.mode == "test":
        test(workflow)
        cross_run_tests()
        transport_tests(workflow)
        workflow_tests()
        toolchain_tests()
    else:
        context = live_context(args.phase, args.predecessor_run_id)
        if args.mode == "prepare":
            prepare(context, workflow)
        else:
            record(context)


if __name__ == "__main__":
    main()
