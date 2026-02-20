Feature: Resumable publishing
  Shipper provides resumable publishing so that interrupted publish runs can be
  continued without re-publishing already completed packages.

  Background:
    Given a workspace with a single publishable crate "demo" version "0.1.0"
    And a fake registry that returns "missing" for "demo@0.1.0" initially

  Scenario: Successful publish writes state and receipt
    Given cargo publish succeeds
    And the registry returns "published" for "demo@0.1.0" after publish
    When I run "shipper publish" with "--verify-timeout 0ms --verify-poll 0ms"
    Then the exit code is 0
    And the state file exists
    And the receipt file exists
    And the receipt shows package "demo@0.1.0" in state "Published"

  Scenario: Resume skips cargo publish when state is Uploaded
    Given an existing state file marks "demo@0.1.0" as "Uploaded"
    And the registry returns "published" for "demo@0.1.0"
    When I run "shipper resume"
    Then the exit code is 0
    And cargo publish was not invoked
    And the receipt shows package "demo@0.1.0" in state "Published"

  Scenario: Already-published crate is skipped
    Given the registry returns "published" for "demo@0.1.0"
    When I run "shipper publish"
    Then the receipt shows package "demo@0.1.0" in state "Skipped"
    And the skip reason contains "already published"