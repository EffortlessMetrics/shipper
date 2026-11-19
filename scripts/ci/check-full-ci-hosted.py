"""Check ci.yml's bounded hosted routes; actionlint owns YAML syntax.

This dependency-free checker deliberately accepts the repository's explicit job
and anchor layout. It checks routing, dependency and shared-command ownership,
not arbitrary YAML. Changes to that layout require updating these predicates.
"""

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
from tempfile import TemporaryDirectory


HOSTED = (
    "github.repository == 'EffortlessMetrics/shipper-swarm' && "
    "github.event_name == 'workflow_dispatch' && "
    "inputs.runner_route == 'github-hosted'"
)
REVIEW = (
    "github.repository == 'EffortlessMetrics/shipper-swarm' && "
    "github.event_name == 'pull_request' && "
    "github.event.pull_request.draft == false && "
    "github.event.pull_request.head.repo.full_name == github.repository"
)
DEFAULT = "${{ github.event_name != 'pull_request' && !(" + HOSTED + ") }}"
POLICY = "${{ (" + HOSTED + ") || (" + REVIEW + ") }}"
MANUAL = "${{ " + HOSTED + " }}"
JOBS = {
    "lint": ("cx43", 30),
    "policy": ("cx43", 20),
    "test": ("cx43", 75),
    "install-smoke": ("cx43", 30),
    "crypto-proptests-heavy": ("cx53", 90),
    "msrv": ("cx43", 30),
    "security": ("cx43", 10),
    "docs": ("cx43", 30),
    "bdd": ("cx43", 45),
    "fuzz-smoke": ("cx53", 30),
    "cross-platform": ("cx43", 30),
    "release-build": ("cx53", 60),
}
DEPENDENCIES = {
    "bdd": ["lint"],
    "fuzz-smoke": ["lint"],
    "release-build": ["lint", "test"],
}
PREDICATES = {
    "crypto-proptests-heavy": "github.event_name == 'schedule' || github.event_name == 'push' || github.event_name == 'workflow_dispatch'",
    "fuzz-smoke": "github.event_name != 'schedule'",
    "release-build": "github.event_name == 'push' || github.event_name == 'workflow_dispatch'",
}
CACHE_PREFIX = "${{ runner.environment == 'github-hosted' && 'manual-hosted-' || '' }}"
POLICY_CHECKS = (
    "python3 scripts/ci/check-full-ci-hosted.py --test",
    "cargo xtask check-file-policy --mode blocking-allowlist",
    "cargo xtask check-generated --mode blocking-allowlist",
    "cargo xtask check-executable-files --mode blocking-allowlist",
    "cargo xtask check-dependency-surfaces --mode blocking-allowlist",
    "cargo xtask check-workflow-surfaces --mode blocking-allowlist",
    "cargo xtask check-process-policy --mode blocking-allowlist",
    "cargo xtask check-network-policy --mode blocking-allowlist",
    "cargo xtask check-doc-contracts --mode advisory",
)


def sections(text, indentation):
    """Read explicit mapping fields at one indentation, retaining nested text."""
    starts = list(re.finditer(r"^" + " " * indentation + r"([\w-]+):([^\n]*)", text, re.M))
    result = {}
    for index, match in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(text)
        if match[1] in result:
            raise ValueError(f"duplicate field: {match[1]}")
        value = text[match.start():end].split(":", 1)[1]
        value = "\n".join(
            line for line in value.splitlines() if not line.lstrip().startswith("#")
        ).rstrip()
        result[match[1]] = value[1:] if value.startswith("\n") else value.lstrip()
    return result


def scalar(value, aliases):
    if value.startswith("*"):
        return aliases.get(value[1:], "unknown alias")
    value = re.sub(r"^&[\w-]+\s+", "", value)
    return " ".join(value.removeprefix(">-").split())


def job_steps(jobs, job):
    steps = sections(jobs.get(job, ""), 4).get("steps", "")
    return [
        sections("        " + body, 8)
        for body in re.split(r"^      - ", steps, flags=re.M)[1:]
    ]


def check(text):
    failures = []

    def require(condition, message):
        if not condition:
            failures.append(message)

    document = sections(text, 0)
    jobs = sections(document.get("jobs", ""), 2)
    expected = set(JOBS) | {job + "-hosted" for job in JOBS}
    require(set(jobs) == expected, "full CI must retain exactly twelve job pairs")
    trigger = document.get("on", "")
    events = sections(trigger, 2)
    pull_request = sections(events.get("pull_request", ""), 4)
    require(pull_request == {
        "branches": "[main]",
        "types": "[opened, synchronize, reopened, ready_for_review]",
    }, "review-ready policy filters must belong to pull_request without narrowing paths")
    require("default: self-hosted" in trigger, "manual input must default to self-hosted")
    require("type: choice" in trigger and "- github-hosted" in trigger, "manual route must be an explicit choice")
    require(document.get("permissions", "").strip() == "contents: read", "CI must retain read-only permissions")
    expected_group = "ci-${{ github.ref }}${{ " + HOSTED + " && '-manual-hosted' || '' }}"
    concurrency = sections(document.get("concurrency", ""), 2)
    require(concurrency.get("group") == expected_group, "manual hosted concurrency must be isolated from default runs")
    require(concurrency.get("cancel-in-progress") == "true", "repeated runs must remain cancellable")
    require("continue-on-error:" not in text, "full CI must not waive failed checks")
    aliases = {"default_runners": DEFAULT, "manual_hosted": MANUAL}

    for job, (capacity, timeout) in JOBS.items():
        original = sections(jobs.get(job, ""), 4)
        hosted = sections(jobs.get(job + "-hosted", ""), 4)
        gate = "${{ github.event_name != 'pull_request' && !(" + HOSTED + ") && (" + PREDICATES[job] + ") }}" if job in PREDICATES else DEFAULT
        require(scalar(original.get("if", ""), aliases) == gate, f"{job}: default route/predicate changed")
        require(scalar(hosted.get("if", ""), aliases) == (POLICY if job == "policy" else MANUAL),
                f"{job}: hosted route must retain explicit manual or trusted ready-PR policy admission")
        if job == "lint":
            require(original.get("if", "").startswith("&default_runners "), "default route anchor must exist")
            require(hosted.get("if", "").startswith("&manual_hosted "), "manual route anchor must exist")
        tier = "rust-large" if capacity == "cx53" else "rust-medium"
        expected_runner = f"group: em-ci-small labels: [self-hosted, linux, x64, em-ci, {capacity}, {tier}, trusted-pr]"
        require(" ".join(original.get("runs-on", "").split()) == expected_runner, f"{job}: original scoped runner changed")
        require(hosted.get("runs-on") == "ubuntu-24.04", f"{job}: hosted route must allocate an ephemeral Ubuntu runner")
        require(hosted.get("timeout-minutes") == str(timeout), f"{job}: hosted runtime budget changed")
        anchor = job.replace("-", "_") + "_steps"
        require(original.get("steps", "").startswith("&" + anchor + "\n"), f"{job}: shared steps must be defined once")
        require(hosted.get("steps") == "*" + anchor, f"{job}: hosted steps must alias the complete original proof")
        deps = DEPENDENCIES.get(job)
        require(original.get("needs") == ("[" + ", ".join(deps) + "]" if deps else None), f"{job}: original dependencies changed")
        require(hosted.get("needs") == ("[" + ", ".join(d + "-hosted" for d in deps) + "]" if deps else None), f"{job}: hosted dependencies must use selected hosted jobs")
        allowed = {"name", "if", "runs-on", "timeout-minutes", "steps"}
        if deps:
            allowed.add("needs")
        if job == "cross-platform":
            allowed.add("strategy")
            require(original.get("strategy", "").startswith("&target_strategy\n"), "target strategy anchor missing")
            require(hosted.get("strategy") == "*target_strategy", "hosted target matrix must alias the original")
        require(set(hosted) == allowed, f"{job}: hosted job cannot override shared proof environment or permissions")
        for cache in re.findall(r"^\s+(?:key|restore-keys): (.+)$", original.get("steps", ""), re.M):
            require(cache.startswith(CACHE_PREFIX), f"{job}: hosted cache must have its own namespace")

    # Inspect actual step fields, not strings that may occur in a disabled
    # step, an echo, or a command whose failure is swallowed. This remains a
    # bounded source-layout oracle; actionlint owns general YAML validation.
    steps = job_steps(jobs, "policy")
    for command in POLICY_CHECKS:
        matching = [step for step in steps if step.get("run") == command]
        require(len(matching) == 1 and set(matching[0]) == {"name", "run"},
                f"policy check must be one unconditional exact command: {command}")

    reports = [step for step in steps if step.get("run") == "cargo xtask policy-report"]
    require(len(reports) == 1 and set(reports[0]) == {"name", "if", "run"}
            and reports[0].get("if") == "always()",
            "unified policy report must retain its exact command and always condition")
    uploads = [step for step in steps if step.get("uses", "").startswith("actions/upload-artifact@")]
    require(len(uploads) == 1 and set(uploads[0]) == {"name", "if", "uses", "with"}
            and uploads[0].get("uses") == "actions/upload-artifact@v7"
            and uploads[0].get("if") == "always()"
            and sections(uploads[0].get("with", ""), 10) == {
                "name": "policy-reports",
                "path": "target/policy/",
                "if-no-files-found": "error",
                "retention-days": "30",
            }, "policy artifact must always retain the canonical report directory without missing-file waivers")

    # The dedicated MSRV job is shared by both routes. Match an ordered, live
    # checkout/install/proof sequence rather than accepting echoed commands.
    steps = job_steps(jobs, "msrv")
    checkout = {"uses": "actions/checkout@v7.0.1", "id": "msrv-checkout"}
    install = {
        "name": "Install Rust (MSRV)",
        "uses": "dtolnay/rust-toolchain@v1",
        "with": "          toolchain: 1.95.0",
    }
    proof = [step for step in steps if step.get("name") == "Check locked workspace with Rust 1.95.0"]
    require(len(steps) == 4 and steps[:2] == [checkout, install]
            and set(steps[2]) == {"name", "uses", "with"}
            and steps[2]["uses"] == "actions/cache@v6",
            "MSRV must check out the event source and install the exact compiler before its cache and proof")
    require(len(proof) == 1 and steps[-1] == proof[0]
            and set(proof[0]) == {"name", "env", "run"}
            and sections(proof[0]["env"], 10) == {
                "CHECKOUT_SHA": "${{ steps.msrv-checkout.outputs.commit }}",
            }
            and [line.strip() for line in proof[0]["run"].splitlines()] == r"""|
set -euo pipefail
printf 'checkout_sha=%s\n' "$CHECKOUT_SHA"
[[ -n "$CHECKOUT_SHA" && "$CHECKOUT_SHA" == "$GITHUB_SHA" ]]
sha256sum Cargo.lock
RUSTC=$(rustup which --toolchain 1.95.0 rustc)
export RUSTC CARGO_BUILD_RUSTC="$RUSTC"
export RUSTC_WRAPPER="" RUSTC_WORKSPACE_WRAPPER=""
export CARGO_BUILD_RUSTC_WRAPPER="" CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER=""
rustc_version=$("$RUSTC" -vV)
printf '%s\n' "$rustc_version"
[[ $'\n'"$rustc_version"$'\n' == *$'\nrelease: 1.95.0\n'* ]]
cargo +1.95.0 -vV
cargo +1.95.0 check --workspace --locked""".splitlines(),
            "MSRV proof must run unconditionally and bind checkout, compiler and locked check")

    # Pin the high-cost/deep proof that a nominally equivalent fallback might omit.
    for witness in [
        "cargo nextest run --workspace --all-features --profile ci",
        'PROPTEST_CASES: "256"',
        "cargo test -p shipper-cli --test e2e_rehearse -- --nocapture",
        "cargo build --release",
        "cargo test --workspace --doc",
        "cargo check --workspace --target ${{ matrix.target }}",
    ]:
        require(witness in text, f"required full-CI proof missing: {witness}")
    return failures


def test_mutations(text):
    # These mutations change the actual workflow, not a mock expression.
    D1_REMOVED = DEFAULT.replace("github.event_name != 'pull_request' && ", "")
    mutants = [
        ("readiness event removed", "ready_for_review", "converted_to_draft"),
        ("default PR route leaked", DEFAULT, D1_REMOVED),
        ("draft policy allowed", "github.event.pull_request.draft == false", "true"),
        ("fork policy allowed", "github.event.pull_request.head.repo.full_name == github.repository", "true"),
        ("policy check removed", "run: cargo xtask check-process-policy --mode blocking-allowlist", "run: true"),
        ("review policy disabled", POLICY, MANUAL),
        ("default route", "default: self-hosted", "default: github-hosted"),
        ("automatic hosted", MANUAL, MANUAL.replace("'workflow_dispatch'", "'push'")),
        ("unscoped hosted", "runs-on: ubuntu-24.04", "runs-on: self-hosted"),
        ("partial commands", "steps: *test_steps", "steps: *lint_steps"),
        ("skipped dependency", "needs: [lint-hosted, test-hosted]", "needs: [lint, test]"),
        ("matrix drift", "strategy: *target_strategy", "strategy: *other_matrix"),
        ("environment override", "steps: *lint_steps", 'env:\n      PROPTEST_CASES: "1"\n    steps: *lint_steps'),
        ("permission expansion", "contents: read", "contents: write"),
        ("weakened deep proof", 'PROPTEST_CASES: "256"', 'PROPTEST_CASES: "16"'),
        ("cache mixing", CACHE_PREFIX, ""),
        ("missing job", "  release-build-hosted:", "  omitted-release-proof:"),
        ("unbounded runtime", "timeout-minutes: 75", "timeout-minutes: 360"),
        ("concurrency collision", " && '-manual-hosted' || ''", " && '' || ''"),
        ("unpinned MSRV installation", "toolchain: 1.95.0", "toolchain: stable"),
        ("unpinned MSRV check", "cargo +1.95.0 check --workspace --locked", "cargo check --workspace --locked"),
        ("unlocked MSRV check", "cargo +1.95.0 check --workspace --locked", "cargo +1.95.0 check --workspace"),
        ("disabled MSRV check", "name: Check locked workspace with Rust 1.95.0",
         "name: Check locked workspace with Rust 1.95.0\n        if: false"),
        ("misbound MSRV checkout", "steps.msrv-checkout.outputs.commit", "github.sha"),
        ("MSRV compiler assertion removed", "release: 1.95.0", "release:"),
        ("missing MSRV lock digest", "sha256sum Cargo.lock", "true"),
        ("MSRV compiler not bound", 'export RUSTC CARGO_BUILD_RUSTC="$RUSTC"', "export CARGO_BUILD_RUSTC"),
        ("MSRV compiler probe drift", 'rustc_version=$("$RUSTC" -vV)', "rustc_version=$(rustc -vV)"),
        ("MSRV wrappers retained", 'export RUSTC_WRAPPER="" RUSTC_WORKSPACE_WRAPPER=""',
         "export RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER"),
        ("waived failure", "steps: *lint_steps", "continue-on-error: true\n    steps: *lint_steps"),
    ]
    mutants.extend([
        ("wrong PR target", "  pull_request:\n    branches: [main]", "  pull_request:\n    branches: [never-policy-target]"),
        ("missing PR target", "  pull_request:\n    branches: [main]", "  pull_request:"),
        ("narrowed PR paths", "  pull_request:\n    branches: [main]", "  pull_request:\n    paths: [only-this-path]\n    branches: [main]"),
        ("disabled report", "name: Unified policy report\n        if: always()", "name: Unified policy report\n        if: false"),
        ("success-only report", "name: Unified policy report\n        if: always()", "name: Unified policy report\n        if: success()"),
        ("masked report failure", "run: cargo xtask policy-report", "run: cargo xtask policy-report || true"),
        ("disabled upload", "name: Upload policy reports\n        if: always()", "name: Upload policy reports\n        if: false"),
        ("success-only upload", "name: Upload policy reports\n        if: always()", "name: Upload policy reports\n        if: success()"),
        ("wrong report name", "name: policy-reports", "name: unrelated-reports"),
        ("wrong report path", "path: target/policy/", "path: target/other/"),
        ("waived missing reports", "if-no-files-found: error", "if-no-files-found: warn"),
        ("changed report retention", "retention-days: 30", "retention-days: 1"),
    ])
    for command in POLICY_CHECKS:
        run = "run: " + command
        mutants.extend([
            ("disabled " + command, run, "if: false\n        " + run),
            ("masked " + command, run, run + " || true"),
            ("echoed " + command, run, 'run: |\n          echo "' + run + '"'),
        ])
    for name, old, new in mutants:
        if old not in text:
            raise ValueError(f"mutation fixture no longer reaches {name}")
        if not check(text.replace(old, new, 1)):
            raise ValueError(f"checker accepted invalid fixture: {name}")
    print(f"Passed {len(mutants)} rejecting workflow fixtures.")


def test_msrv_invocation(text):
    jobs = sections(sections(text, 0)["jobs"], 2)
    script = job_steps(jobs, "msrv")[-1]["run"].split("\n", 1)[1]
    with TemporaryDirectory() as directory:
        root = Path(directory)
        # A space in the compiler pathname also checks that the probe quotes it.
        compiler = root / "toolchain rustc"
        stubs = {
            "rustup": """[[ "$*" == "which --toolchain 1.95.0 rustc" ]] || exit 10
printf '%s\\n' "$TEST_COMPILER" """,
            "toolchain rustc": """[[ "$*" == "-vV" ]] || exit 11
printf '%s\\n' 'release: 1.95.0'""",
            "rustc": "printf '%s\\n' 'release: 1.95.0'",
            "cargo": """[[ "$RUSTC" == "$TEST_COMPILER" && "$CARGO_BUILD_RUSTC" == "$TEST_COMPILER" ]] || exit 12
[[ -z "$RUSTC_WRAPPER" && -z "$RUSTC_WORKSPACE_WRAPPER" ]] || exit 13
[[ -z "$CARGO_BUILD_RUSTC_WRAPPER" && -z "$CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER" ]] || exit 14
printf '%s\\n' "$*" >> cargo-invocations""",
        }
        for name, body in stubs.items():
            path = root / name
            path.write_text("#!/bin/bash\n" + body + "\n", encoding="utf-8")
            path.chmod(0o755)
        (root / "Cargo.lock").write_text("fixture lockfile\n", encoding="utf-8")
        env = dict(os.environ, PATH=f"{root}{os.pathsep}{os.environ['PATH']}",
                   CHECKOUT_SHA="a" * 40, GITHUB_SHA="a" * 40, TEST_COMPILER=str(compiler))
        for key in ("RUSTC", "CARGO_BUILD_RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                    "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"):
            env[key] = "/fixture/ambient-override"
        result = subprocess.run(["bash", "-c", script], cwd=root, env=env, capture_output=True, text=True)
        calls = root / "cargo-invocations"
        if result.returncode or not calls.exists() or calls.read_text(encoding="utf-8").splitlines() != [
            "+1.95.0 -vV", "+1.95.0 check --workspace --locked",
        ]:
            raise ValueError(f"MSRV proof did not bind pinned rustc and reset inherited wrappers: {result.stderr}")
    print("Passed MSRV invocation fixture with conflicting ambient compiler and wrappers.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true", help="also exercise rejecting workflow fixtures")
    parser.add_argument("--workflow", type=Path, default=Path(".github/workflows/ci.yml"))
    args = parser.parse_args()
    text = args.workflow.read_text(encoding="utf-8")
    failures = check(text)
    if failures:
        for failure in failures:
            print(f"ERROR: {failure}", file=sys.stderr)
        return 1
    print("Hosted routes, twelve job pairs, executable policy proof, MSRV receipt and report retention verified.")
    if args.test:
        test_mutations(text)
        test_msrv_invocation(text)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
