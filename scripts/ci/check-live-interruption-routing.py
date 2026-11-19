"""Check the explicit interruption/resume job layout; actionlint owns YAML syntax.

Reuse the repository's dependency-free layout parser. This checks the supported
workflow shape and its artifact boundary, not arbitrary YAML or Actions code.
"""

import argparse
from pathlib import Path
import re
import runpy
import sys


LAYOUT = runpy.run_path(str(Path(__file__).with_name("check-full-ci-hosted.py")))
sections = LAYOUT["sections"]
scalar = LAYOUT["scalar"]
TRUSTED = "(github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository)"
SWARM = "github.repository == 'EffortlessMetrics/shipper-swarm' && " + TRUSTED
SOURCE = "github.repository != 'EffortlessMetrics/shipper-swarm' && " + TRUSTED
CACHE = "${{ runner.environment == 'github-hosted' && 'hosted-' || '' }}"
ROOT = "${{ runner.temp }}/shipper-live-interruption"
SEED = "shipper-live-interruption-seed-${{ github.run_id }}"
RESUMED = "shipper-live-interruption-resume-${{ github.run_id }}"
TEST_PREFIX = "cargo test -p shipper-cli --test e2e_rehearse "
TEST_SUFFIX = " -- --ignored --exact --nocapture"
TESTS = {
    "interrupt": "live_runner_interruption_seed_uploads_shipper_artifact",
    "resume": "live_runner_interruption_resume_downloaded_artifact_preserves_invariants",
}


def check(text):
    failures = []

    def require(condition, message):
        if not condition:
            failures.append(message)

    document = sections(text, 0)
    jobs = sections(document.get("jobs", ""), 2)
    require(set(jobs) == {"interrupt", "resume", "interrupt-hosted", "resume-hosted"}, "exactly two route pairs are required")
    require(document.get("permissions", "").strip() == "contents: read", "permissions must remain read-only")
    require("secrets." not in text and "continue-on-error:" not in text, "rehearsal must not receive secrets or waive failures")
    trigger = document.get("on", "")
    for path in [".github/workflows/live-runner-interruption-rehearsal.yml", "scripts/ci/check-live-interruption-routing.py", "scripts/ci/check-full-ci-hosted.py", "crates/shipper-cli/tests/e2e_rehearse.rs", "crates/shipper-core/src/engine/**", "crates/shipper-core/src/state/**"]:
        require(f'      - "{path}"' in trigger, f"missing proof trigger: {path}")
    require(set(sections(trigger, 2)) == {"workflow_dispatch", "pull_request"}, "rehearsal triggers must remain dispatch and path-scoped PR")
    aliases = {"self_hosted": SOURCE, "swarm_hosted": SWARM}

    for phase in TESTS:
        original = sections(jobs.get(phase, ""), 4)
        hosted = sections(jobs.get(phase + "-hosted", ""), 4)
        require(scalar(original.get("if", ""), aliases) == SOURCE, f"{phase}: source route or fork refusal changed")
        require(scalar(hosted.get("if", ""), aliases) == SWARM, f"{phase}: hosted route must be swarm-only and refuse forks")
        if phase == "interrupt":
            require(original.get("if", "").startswith("&self_hosted "), "source guard anchor missing")
            require(hosted.get("if", "").startswith("&swarm_hosted "), "swarm guard anchor missing")
        expected_runner = "group: em-ci-small labels: [self-hosted, linux, x64, em-ci, cx43, rust-medium, trusted-pr]"
        require(" ".join(original.get("runs-on", "").split()) == expected_runner, f"{phase}: original scoped source runner changed")
        require(hosted.get("runs-on") == "ubuntu-24.04", f"{phase}: hosted runner changed")
        require(hosted.get("timeout-minutes") == "30", f"{phase}: hosted runtime must be bounded")
        require(original.get("steps", "").startswith("&" + phase + "_steps\n"), f"{phase}: original steps must be the shared authority")
        require(hosted.get("steps") == "*" + phase + "_steps", f"{phase}: hosted steps must alias the complete original sequence")
        require(original.get("needs") == ("interrupt" if phase == "resume" else None), f"{phase}: source dependency changed")
        require(hosted.get("needs") == ("interrupt-hosted" if phase == "resume" else None), f"{phase}: hosted resume must depend on selected interrupt")
        expected_fields = {"name", "if", "runs-on", "steps"} | ({"needs"} if phase == "resume" else set())
        require(set(original) == expected_fields, f"{phase}: source job has an unexpected override")
        require(set(hosted) == expected_fields | {"timeout-minutes"}, f"{phase}: hosted job has an unexpected override")

        steps = original.get("steps", "")
        require(re.findall(r"^\s+(cargo .+)$", steps, re.M) == [TEST_PREFIX + TESTS[phase] + TEST_SUFFIX], f"{phase}: exact ignored test command changed")
        for cache in re.findall(r"^\s+(?:key|restore-keys): (.+)$", steps, re.M):
            require(cache.startswith(CACHE), f"{phase}: hosted/source cache mixing")
        named_steps = dict(re.findall(r"(?ms)^      - name: ([^\n]+)\n(.*?)(?=^      - |\Z)", steps))
        test_name = "Create interrupted .shipper artifact" if phase == "interrupt" else "Resume from downloaded .shipper"
        test = sections(named_steps.get(test_name, ""), 8)
        require(set(test) == {"env", "run"}, f"{phase}: test must execute without a condition or error waiver")
        require(test.get("run", "").removeprefix("|").strip() == TEST_PREFIX + TESTS[phase] + TEST_SUFFIX, f"{phase}: complete run block must execute only the required Cargo test")
        require(sections(test.get("env", ""), 10) == {"SHIPPER_LIVE_REHEARSAL_ROOT": ROOT, "SHIPPER_LIVE_REHEARSAL_REGISTRY_ADDR": "127.0.0.1:39197"}, f"{phase}: test must use the retained root and mock registry")

        uploads = [("Upload interrupted .shipper", SEED)] if phase == "interrupt" else [("Upload resumed .shipper", RESUMED)]
        for name, artifact in uploads:
            upload = sections(named_steps.get(name, ""), 8)
            require(set(upload) == {"if", "uses", "with"} and upload.get("if") == "always()", f"{phase}: evidence upload must run even after test failure")
            require(upload.get("uses", "").startswith("actions/upload-artifact@"), f"{phase}: real artifact upload action missing")
            require(sections(upload.get("with", ""), 10) == {"name": artifact, "path": ROOT + "/.shipper/", "include-hidden-files": "true", "retention-days": "30"}, f"{phase}: hidden artifact upload contract changed")
        if phase == "resume":
            download = sections(named_steps.get("Download interrupted .shipper", ""), 8)
            require(set(download) == {"uses", "with"} and download.get("uses", "").startswith("actions/download-artifact@"), "resume must execute real download without a condition")
            require(sections(download.get("with", ""), 10) == {"name": SEED, "path": ROOT + "/.shipper/"}, "resume must download this run's seed artifact into the expected root")
            require(steps.find("- name: Download interrupted .shipper") < steps.find("- name: Resume from downloaded .shipper"), "download must precede resume")
        else:
            require("run: python3 scripts/ci/check-live-interruption-routing.py --test" in steps, "workflow must execute its routing fixtures")
    return failures


def test_mutations(text):
    mutants = [
        ("cross-repository hosted route", "github.repository == 'EffortlessMetrics/shipper-swarm' &&", "true &&"),
        ("fork execution", TRUSTED, "true"),
        ("unselected dependency", "needs: interrupt-hosted", "needs: interrupt"),
        ("wrong shared steps", "steps: *resume_steps", "steps: *interrupt_steps"),
        ("source runner drift", "cx43, rust-medium", "cx53, rust-large"),
        ("permission expansion", "contents: read", "contents: write"),
        ("skipped resume test", "      - name: Resume from downloaded .shipper\n", "      - name: Resume from downloaded .shipper\n        if: false\n"),
        ("wrong test", TESTS["resume"], TESTS["interrupt"]),
        ("early interrupt success", "          " + TEST_PREFIX + TESTS["interrupt"], "          exit 0\n          " + TEST_PREFIX + TESTS["interrupt"]),
        ("early resume success", "          " + TEST_PREFIX + TESTS["resume"], "          exit 0\n          " + TEST_PREFIX + TESTS["resume"]),
        ("missing hidden evidence", "include-hidden-files: true", "include-hidden-files: false"),
        ("unrelated seed artifact", "name: " + SEED, "name: unrelated-seed"),
        ("cross-run download", "uses: actions/download-artifact@v8", "uses: actions/upload-artifact@v8"),
        ("real registry", "127.0.0.1:39197", "crates.io:443"),
        ("cache mixing", CACHE, ""),
        ("unbounded job", "timeout-minutes: 30", "timeout-minutes: 360"),
        ("waived failure", "steps: *resume_steps", "continue-on-error: true\n    steps: *resume_steps"),
    ]
    for name, old, new in mutants:
        if old not in text:
            raise ValueError(f"fixture no longer reaches {name}")
        if not check(text.replace(old, new, 1)):
            raise ValueError(f"checker accepted invalid fixture: {name}")
    print(f"Passed {len(mutants)} rejecting rehearsal workflow fixtures.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true")
    parser.add_argument("--workflow", type=Path, default=Path(".github/workflows/live-runner-interruption-rehearsal.yml"))
    args = parser.parse_args()
    text = args.workflow.read_text(encoding="utf-8")
    failures = check(text)
    if failures:
        for failure in failures:
            print(f"ERROR: {failure}", file=sys.stderr)
        return 1
    print("Swarm/source route separation, selected dependencies, shared tests and artifact handoff verified.")
    if args.test:
        test_mutations(text)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
