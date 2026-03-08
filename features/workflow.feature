Feature: Cross-cutting workflow scenarios
  End-to-end BDD scenarios that exercise the resume, parallel publish,
  status, and doctor commands in representative workflow situations.

  # ── Resume workflow ────────────────────────────────────────────────

  Scenario: Resume after interrupted publish completes remaining crates
    Given a workspace with crates "core" and "app" where "app" depends on "core"
    And a prior publish run failed while publishing "app"
    And the state file marks "core@0.1.0" as "Skipped" and "app@0.1.0" as "Failed"
    And cargo publish succeeds for "app"
    And the registry returns "not found" for "app@0.1.0" then "published"
    When I run "shipper resume"
    Then the exit code is 0
    And the receipt shows package "app@0.1.0" in state "Published"
    And cargo publish was not invoked for "core"

  Scenario: Resume with all packages already published is a no-op
    Given a workspace with a single crate "demo" version "0.1.0"
    And a prior publish run succeeded
    And the state file marks "demo@0.1.0" as "Published"
    When I run "shipper resume"
    Then the exit code is 0
    And cargo publish was not invoked
    And the output reports packages as already complete

  # ── Parallel publish ───────────────────────────────────────────────

  Scenario: Parallel publish groups independent crates into one level
    Given a workspace with independent crates "alpha", "beta", and "gamma"
    And the registry reports all versions as already published
    When I run "shipper publish --parallel --max-concurrent 2"
    Then the exit code is 0
    And all three crates appear in the receipt as "Skipped"

  Scenario: Parallel publish respects dependency ordering across levels
    Given a workspace with "core", "api", "cli", and "app"
    And "api" and "cli" depend on "core", "app" depends on both
    And the registry reports all versions as already published
    When I run "shipper publish --parallel"
    Then the exit code is 0
    And all four crates appear in the receipt

  # ── Status command ─────────────────────────────────────────────────

  Scenario: Status reports mixed published and missing crates
    Given a workspace with crates "core", "utils", and "app"
    And the registry returns "published" for "core@0.1.0"
    And the registry returns "not found" for "utils@0.1.0" and "app@0.1.0"
    When I run "shipper status"
    Then the exit code is 0
    And the output contains "core" with status "published"
    And the output contains "utils" and "app" with status "missing"

  Scenario: Status for a single-crate workspace shows version
    Given a workspace with a single publishable crate "solo" version "0.3.0"
    And the registry returns "not found" for "solo@0.3.0"
    When I run "shipper status"
    Then the exit code is 0
    And the output contains "solo@0.3.0"

  # ── Doctor diagnostics ─────────────────────────────────────────────

  Scenario: Doctor reports diagnostics header and workspace root
    Given a valid workspace with crate "demo"
    And a reachable mock registry
    When I run "shipper doctor"
    Then the exit code is 0
    And the output contains "Shipper Doctor - Diagnostics Report"
    And the output contains "workspace_root:"

  Scenario: Doctor warns when no registry token is configured
    Given a valid workspace with crate "demo"
    And a reachable mock registry
    And no CARGO_REGISTRY_TOKEN is set
    When I run "shipper doctor"
    Then the exit code is 0
    And the output contains "NONE FOUND"

  Scenario: Doctor detects cargo version
    Given a valid workspace with crate "demo"
    And a reachable mock registry
    When I run "shipper doctor"
    Then the exit code is 0
    And the output contains "cargo: cargo"

  Scenario: Doctor reports registry reachability
    Given a valid workspace with crate "demo"
    And a reachable mock registry
    When I run "shipper doctor"
    Then the exit code is 0
    And the output contains "registry_reachable: true"

  # ── Config validation workflow ──────────────────────────────────

  Scenario: Config validate rejects zero retry max_attempts
    Given a file ".shipper.toml" with retry max_attempts set to 0
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error output mentions "max_attempts"

  Scenario: Config validate rejects jitter outside valid range
    Given a file ".shipper.toml" with retry jitter set to 1.5
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error output mentions "jitter"

  # ── Doctor token warning ────────────────────────────────────────

  Scenario: Doctor reports token source when no token is configured
    Given a valid workspace with crate "demo"
    And a reachable mock registry
    And no CARGO_REGISTRY_TOKEN is set and no credentials file exists
    When I run "shipper doctor"
    Then the exit code is 0
    And the output contains "auth_type:"
    And the output contains "NONE FOUND"

  # ── Clean command ───────────────────────────────────────────────

  Scenario: Clean removes state files from .shipper directory
    Given a workspace with crate "demo"
    And the state directory contains "state.json" and "events.jsonl"
    When I run "shipper clean"
    Then the exit code is 0
    And the output contains "Clean complete"
    And "state.json" no longer exists in the state directory

  Scenario: Clean with --keep-receipt preserves receipt.json
    Given a workspace with crate "demo"
    And the state directory contains "state.json", "events.jsonl", and "receipt.json"
    When I run "shipper clean --keep-receipt"
    Then the exit code is 0
    And "receipt.json" still exists in the state directory
    And "state.json" no longer exists in the state directory

  # ── Plan with package filter ────────────────────────────────────

  Scenario: Plan with --package filter shows only selected package and its deps
    Given a workspace with "core", "utils", and "app" where "app" depends on both
    When I run "shipper plan --package app"
    Then the exit code is 0
    And the output contains "app@0.1.0"
    And the total packages to publish includes "app" and its dependencies

  # ── Dry run publish (preflight) ─────────────────────────────────

  Scenario: Preflight checks workspace without publishing
    Given a workspace with crate "demo"
    And the registry reports version as already published
    When I run "shipper preflight --allow-dirty"
    Then the exit code is 0
    And no state.json is created in the state directory
    And no cargo publish invocation occurs
