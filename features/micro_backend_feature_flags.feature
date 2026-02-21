Feature: Micro backend feature flags
  Shipper's auth, git, events, lock, encrypt, environment, and storage modules can be provided by shared microcrates.

  Scenario: Preflight behavior stays stable with micro backends enabled
    Given a workspace with a dependency chain
    And no registry token is configured
    And the registry returns "not found" for all crates
    When I run "shipper preflight" with "--policy fast" and "--allow-dirty"
    Then the preflight report shows token not detected
    And the exit code is 0

  Scenario: Preflight behavior stays stable with all micro crates enabled
    Given a workspace with a dependency chain
    And no registry token is configured
    And the registry returns "not found" for all crates
    When I run "shipper preflight" with "--policy fast" and "--allow-dirty"
    Then the preflight report shows token not detected
    And the exit code is 0
