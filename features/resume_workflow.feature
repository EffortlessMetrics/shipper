Feature: Resume workflow
  Shipper persists publish progress to .shipper/state.json so that
  interrupted runs can be resumed without re-publishing completed packages.

  Background:
    Given a workspace with crates "core" and "app" where "app" depends on "core"

  Scenario: Resume continues from where publish was interrupted
    Given an existing state file marks "core@0.1.0" as "Published"
    And an existing state file marks "app@0.1.0" as "Pending"
    And the registry returns "published" for "core@0.1.0"
    And the registry returns "not found" for "app@0.1.0"
    And cargo publish succeeds for "app"
    When I run "shipper resume"
    Then the exit code is 0
    And cargo publish was not invoked for "core"
    And the receipt shows package "app@0.1.0" in state "Published"

  Scenario: Resume skips cargo publish for Uploaded packages
    Given an existing state file marks "core@0.1.0" as "Uploaded"
    And the registry returns "published" for "core@0.1.0"
    When I run "shipper resume"
    Then cargo publish was not invoked for "core"
    And the receipt shows package "core@0.1.0" in state "Published"

  Scenario: Resume rejects mismatched plan_id
    Given an existing state file with plan_id "abc123"
    And the current workspace generates plan_id "def456"
    When I run "shipper resume"
    Then the exit code is non-zero
    And the error message contains "plan_id"

  Scenario: Force resume bypasses plan_id mismatch
    Given an existing state file with plan_id "abc123"
    And the current workspace generates plan_id "def456"
    And cargo publish succeeds
    When I run "shipper resume" with "--force-resume"
    Then the exit code is 0
    And a warning about plan_id mismatch is emitted

  Scenario: Resume from a specific package
    Given an existing state file marks "core@0.1.0" as "Failed"
    And an existing state file marks "app@0.1.0" as "Pending"
    And cargo publish succeeds for "core"
    And the registry returns "not found" for "core@0.1.0"
    When I run "shipper resume" with "--resume-from core"
    Then cargo publish is invoked for "core"
    And the exit code is 0

  Scenario: Resume with no state file errors cleanly
    Given no state file exists
    When I run "shipper resume"
    Then the exit code is non-zero
    And the error message mentions missing state

  Scenario: Resume treats Published packages as complete
    Given an existing state file marks "core@0.1.0" as "Published"
    And an existing state file marks "app@0.1.0" as "Published"
    When I run "shipper resume"
    Then the exit code is 0
    And cargo publish was not invoked
    And the receipt shows all packages as "Published"

  Scenario: Resume treats Skipped packages as complete
    Given an existing state file marks "core@0.1.0" as "Skipped"
    And an existing state file marks "app@0.1.0" as "Pending"
    And cargo publish succeeds for "app"
    And the registry returns "not found" for "app@0.1.0"
    When I run "shipper resume"
    Then cargo publish was not invoked for "core"
    And cargo publish was invoked for "app"

  Scenario: State file is updated atomically during resume
    Given an existing state file marks "core@0.1.0" as "Pending"
    And cargo publish succeeds for "core"
    And the registry returns "published" for "core@0.1.0" after publish
    When I run "shipper resume"
    Then the state file is valid JSON after completion
    And the state file reflects all updated package states
