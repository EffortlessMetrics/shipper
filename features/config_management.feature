Feature: Configuration management
  Shipper supports project-specific configuration via .shipper.toml with
  init, validate, and merge capabilities. CLI flags always take precedence
  over config file values, which take precedence over defaults.

  Scenario: Config init creates a default .shipper.toml
    Given an empty workspace directory
    When I run "shipper config init"
    Then the exit code is 0
    And the file ".shipper.toml" exists
    And the file contains a "[policy]" section
    And the file contains a "[retry]" section
    And the file contains a "[verify]" section

  Scenario: Config init writes to custom output path
    Given an empty workspace directory
    When I run "shipper config init" with "-o custom-config.toml"
    Then the exit code is 0
    And the file "custom-config.toml" exists
    And the file "custom-config.toml" contains a "[policy]" section

  Scenario: Config validate accepts a valid config file
    Given a file ".shipper.toml" with valid configuration
    When I run "shipper config validate"
    Then the exit code is 0

  Scenario: Config validate accepts custom path
    Given a file "configs/release.toml" with valid configuration
    When I run "shipper config validate" with "-p configs/release.toml"
    Then the exit code is 0

  Scenario: Config validate rejects zero retry max_attempts
    Given a file ".shipper.toml" with retry max_attempts set to 0
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error message mentions "max_attempts"

  Scenario: Config validate rejects base_delay greater than max_delay
    Given a file ".shipper.toml" with retry base_delay "5m" and max_delay "30s"
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error message mentions delay ordering

  Scenario: Config validate rejects jitter outside valid range
    Given a file ".shipper.toml" with retry jitter set to 1.5
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error message mentions "jitter"

  Scenario: CLI flag overrides config file policy
    Given a file ".shipper.toml" with policy mode set to "safe"
    When I run "shipper preflight" with "--policy fast" and "--allow-dirty"
    Then the effective policy is "fast"

  Scenario: Config file values override defaults
    Given a file ".shipper.toml" with retry max_attempts set to 10
    When the configuration is loaded
    Then the effective max_attempts is 10
    And it differs from the default value of 6

  Scenario: Config validate rejects invalid schema version
    Given a file ".shipper.toml" with schema_version "shipper.config.v999"
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error message mentions schema version

  Scenario: Config validate rejects zero parallel max_concurrent
    Given a file ".shipper.toml" with parallel max_concurrent set to 0
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error message mentions "max_concurrent"

  Scenario: Config validate rejects multiple default registries
    Given a file ".shipper.toml" with two registries both marked as default
    When I run "shipper config validate"
    Then the exit code is non-zero
    And the error message mentions "default" registry
