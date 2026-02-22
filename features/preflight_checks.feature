Feature: Preflight verification
  Shipper runs preflight checks before publishing to ensure the publish will
  succeed and the user has appropriate permissions.

  Background:
    Given a workspace with crates "core" and "app" where "app" depends on "core"

  Scenario: Preflight passes for new crates with token
    Given a valid registry token is configured
    And the registry returns "not found" for all crates
    When I run "shipper preflight"
    Then the exit code is 0
    And the preflight report shows finishability "Proven"
    And all packages are marked as new crates

  Scenario: Preflight detects already published versions
    Given a valid registry token is configured
    And the registry returns "published" for "core@0.1.0"
    And the registry returns "not found" for "app@0.1.0"
    When I run "shipper preflight"
    Then the preflight report shows "core@0.1.0" as already published
    And the preflight report shows "app@0.1.0" as not published

  Scenario: Preflight warns on missing token
    Given no registry token is configured
    And the registry returns "not found" for all crates
    When I run "shipper preflight" with "--policy fast"
    Then the preflight report shows token not detected
    And the exit code is 0

  Scenario: Preflight fails with dirty git tree
    Given a valid registry token is configured
    And the git working tree has uncommitted changes
    When I run "shipper preflight" without "--allow-dirty"
    Then the exit code is non-zero
    And the error message contains "dirty"

  Scenario: Strict ownership check fails without token
    Given no registry token is configured
    And the registry returns "not found" for all crates
    When I run "shipper preflight" with "--strict-ownership"
    Then the exit code is non-zero
    And the error message mentions token or ownership