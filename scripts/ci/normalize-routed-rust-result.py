#!/usr/bin/env python3
"""Normalize the routed Rust-small workflow into one blocking result."""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass


SELF_HOSTED_TARGETS = ("cx43", "cpx42", "cx53")
HARD_FAIL_REASONS = {
    "fork_pr",
    "runner_token_missing",
    "runner_token_unauthorized",
    "runner_token_forbidden",
    "runner_api_failed",
    "parse_failed",
}


@dataclass(frozen=True)
class RoutedResult:
    target: str
    reason: str
    route_result: str
    trusted: str
    fallback_allowed: bool
    results: dict[str, str]


def failures_for(result: RoutedResult) -> list[str]:
    """Return blocking failures for one routed workflow result."""

    if result.route_result != "success":
        return ["Route job did not succeed."]
    if result.target not in result.results:
        return ["Route job did not emit a known target."]
    if result.target == "github" and result.reason in HARD_FAIL_REASONS:
        return [
            "Fork PRs cannot run repository code on self-hosted runners."
            if result.reason == "fork_pr"
            else "Self-hosted routing failed before a trustworthy fallback decision."
        ]

    failures = []
    for name, value in result.results.items():
        if name == result.target:
            if value != "success":
                if (
                    name in SELF_HOSTED_TARGETS
                    and value == "cancelled"
                    and result.results["github"] == "success"
                ):
                    continue
                failures.append(f"selected {name} job result was {value}")
        elif value != "skipped":
            if (
                name == "github"
                and result.target in SELF_HOSTED_TARGETS
                and value == "success"
                and result.results[result.target] == "cancelled"
            ):
                continue
            failures.append(f"unselected {name} job result was {value}")
    return failures


def normalize(result: RoutedResult) -> int:
    print(f"router_target={result.target}")
    print(f"router_reason={result.reason}")
    print(f"trusted={result.trusted}")
    print(f"route_result={result.route_result}")
    for name, value in result.results.items():
        print(f"{name}_result={value}")

    failures = failures_for(result)
    for failure in failures:
        print(failure)
    if failures:
        return 1
    print("Exactly one routed Rust small lane succeeded.")
    return 0


def test_cases() -> None:
    cases = [
        (
            "github direct fallback",
            RoutedResult(
                "github", "forced_github", "success", "true", True,
                {"cx43": "skipped", "cpx42": "skipped", "cx53": "skipped", "github": "success"},
            ),
            False,
        ),
        (
            "self-hosted success",
            RoutedResult(
                "cx43", "cx43_idle", "success", "true", False,
                {"cx43": "success", "cpx42": "skipped", "cx53": "skipped", "github": "skipped"},
            ),
            False,
        ),
        (
            "cancelled self-hosted with successful fallback",
            RoutedResult(
                "cx43", "cx43_idle", "success", "true", False,
                {"cx43": "cancelled", "cpx42": "skipped", "cx53": "skipped", "github": "success"},
            ),
            False,
        ),
        (
            "failed self-hosted",
            RoutedResult(
                "cx43", "cx43_idle", "success", "true", False,
                {"cx43": "failure", "cpx42": "skipped", "cx53": "skipped", "github": "skipped"},
            ),
            True,
        ),
        (
            "cancelled self-hosted with failed fallback",
            RoutedResult(
                "cx43", "cx43_idle", "success", "true", False,
                {"cx43": "cancelled", "cpx42": "skipped", "cx53": "skipped", "github": "failure"},
            ),
            True,
        ),
        (
            "hard-fail github routing",
            RoutedResult(
                "github", "runner_token_missing", "success", "true", False,
                {"cx43": "skipped", "cpx42": "skipped", "cx53": "skipped", "github": "skipped"},
            ),
            True,
        ),
    ]
    for name, result, expected_failure in cases:
        actual_failure = bool(failures_for(result))
        assert actual_failure is expected_failure, f"{name}: expected {expected_failure}, got {actual_failure}"


def main() -> int:
    if "--test" in sys.argv[1:]:
        test_cases()
        print("routed result normalization tests passed")
        return 0

    result = RoutedResult(
        target=os.environ.get("TARGET", ""),
        reason=os.environ.get("REASON", ""),
        route_result=os.environ.get("ROUTE_RESULT", ""),
        trusted=os.environ.get("TRUSTED", ""),
        fallback_allowed=os.environ.get("FALLBACK_ALLOWED", "").lower() == "true",
        results={
            "cx43": os.environ.get("CX43_RESULT", ""),
            "cpx42": os.environ.get("CPX42_RESULT", ""),
            "cx53": os.environ.get("CX53_RESULT", ""),
            "github": os.environ.get("GITHUB_RESULT", ""),
        },
    )
    return normalize(result)


if __name__ == "__main__":
    raise SystemExit(main())
