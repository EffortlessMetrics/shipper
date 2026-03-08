Feature: Status command
  Shipper's status command compares local workspace versions against the
  registry, reporting which crates are published, missing, or outdated.

  Background:
    Given a workspace with crates "core", "utils", and "app"
    And "app" depends on "core" and "utils"

  Scenario: All crates are unpublished
    Given the registry returns "not found" for all crates
    When I run "shipper status"
    Then the exit code is 0
    And the output contains "core@0.1.0: missing"
    And the output contains "utils@0.1.0: missing"
    And the output contains "app@0.1.0: missing"

  Scenario: All crates are already published
    Given the registry returns "published" for "core@0.1.0"
    And the registry returns "published" for "utils@0.1.0"
    And the registry returns "published" for "app@0.1.0"
    When I run "shipper status"
    Then the exit code is 0
    And the output contains "core@0.1.0: published"
    And the output contains "utils@0.1.0: published"
    And the output contains "app@0.1.0: published"

  Scenario: Mixed published and missing crates
    Given the registry returns "published" for "core@0.1.0"
    And the registry returns "not found" for "utils@0.1.0"
    And the registry returns "not found" for "app@0.1.0"
    When I run "shipper status"
    Then the exit code is 0
    And the output contains "core@0.1.0: published"
    And the output contains "utils@0.1.0: missing"
    And the output contains "app@0.1.0: missing"

  Scenario: Status respects custom manifest path
    Given a workspace at "subdir/Cargo.toml" with crate "leaf" version "0.2.0"
    And the registry returns "not found" for "leaf@0.2.0"
    When I run "shipper status" with "--manifest-path subdir/Cargo.toml"
    Then the exit code is 0
    And the output contains "leaf@0.2.0: missing"

  Scenario: Status with alternative registry
    Given a workspace with crate "internal" version "1.0.0"
    And a custom registry "my-registry" is configured
    And "my-registry" returns "published" for "internal@1.0.0"
    When I run "shipper status" with "--registry my-registry"
    Then the exit code is 0
    And the output contains "internal@1.0.0: published"

  Scenario: Status handles single-crate workspace
    Given a workspace with a single publishable crate "solo" version "0.3.0"
    And the registry returns "not found" for "solo@0.3.0"
    When I run "shipper status"
    Then the exit code is 0
    And the output contains "solo@0.3.0: missing"

  Scenario: Status reports all planned packages in dependency order
    Given the registry returns "published" for "core@0.1.0"
    And the registry returns "not found" for "utils@0.1.0"
    And the registry returns "not found" for "app@0.1.0"
    When I run "shipper status"
    Then "core" appears before "utils" in the output
    And "utils" appears before "app" in the output

  Scenario: Status exits gracefully when registry is unreachable
    Given the registry is unreachable
    When I run "shipper status"
    Then the exit code is non-zero
    And the error message mentions registry connectivity
