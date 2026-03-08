Feature: Error handling and recovery
  Shipper classifies publish failures into Retryable, Permanent, and Ambiguous
  categories, applying appropriate retry strategies for transient errors while
  failing fast on unrecoverable problems.

  Background:
    Given a workspace with a single publishable crate "demo" version "0.1.0"

  Scenario: Auth failure is classified as permanent and not retried
    Given cargo publish output contains "not authorized"
    When publish failure classification runs
    Then the failure class is "Permanent"
    And no retry is attempted

  Scenario: Invalid token is classified as permanent
    Given cargo publish output contains "token is invalid"
    When publish failure classification runs
    Then the failure class is "Permanent"

  Scenario: Rate limiting triggers retry with backoff
    Given cargo publish output contains "429 too many requests"
    When publish failure classification runs
    Then the failure class is "Retryable"
    And the retry delay uses exponential backoff

  Scenario: Connection timeout triggers retry
    Given cargo publish output contains "operation timed out"
    When publish failure classification runs
    Then the failure class is "Retryable"

  Scenario: DNS resolution failure triggers retry
    Given cargo publish output contains "dns error"
    When publish failure classification runs
    Then the failure class is "Retryable"

  Scenario: Server error 502 triggers retry
    Given cargo publish output contains "502 bad gateway"
    When publish failure classification runs
    Then the failure class is "Retryable"

  Scenario: Connection reset triggers retry
    Given cargo publish output contains "connection reset by peer"
    When publish failure classification runs
    Then the failure class is "Retryable"

  Scenario: Compilation failure is permanent
    Given cargo publish output contains "compilation failed"
    When publish failure classification runs
    Then the failure class is "Permanent"

  Scenario: Already-published version is classified as permanent
    Given cargo publish output contains "already exists"
    When publish failure classification runs
    Then the failure class is "Permanent"

  Scenario: Unrecognized error is classified as ambiguous
    Given cargo publish output contains "unexpected registry response: xyz"
    When publish failure classification runs
    Then the failure class is "Ambiguous"
    And the engine checks registry visibility to resolve the ambiguity

  Scenario: Retryable failure exhausts max attempts
    Given cargo publish fails with "connection reset by peer" on every attempt
    And retry policy allows 3 max attempts
    When I run "shipper publish"
    Then cargo publish is invoked 3 times
    And the exit code is non-zero
    And the receipt shows package "demo@0.1.0" in state "Failed"

  Scenario: Ambiguous failure resolves to Published via registry check
    Given cargo publish exits with an unrecognized error
    And the registry returns "published" for "demo@0.1.0"
    When publish failure classification runs and registry is checked
    Then the package is marked as "Published"

  Scenario: Exponential backoff respects max_delay cap
    Given retry config has base_delay "2s" and max_delay "30s"
    When the 10th retry delay is computed
    Then the delay does not exceed "30s"
