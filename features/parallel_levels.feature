Feature: Parallel publish level grouping
  Shipper groups packages into dependency levels so independent crates can
  publish concurrently while preserving dependency order.

  Scenario: Fan-out/fan-in workspace creates three publish levels
    Given a workspace with "core", "api", "cli", and "app"
    And "api" and "cli" depend on "core"
    And "app" depends on both "api" and "cli"
    When plan levels are computed
    Then level 0 contains "core"
    And level 1 contains "api" and "cli"
    And level 2 contains "app"

  Scenario: Independent crates all land in level 0
    Given a workspace with "alpha", "beta", and "gamma"
    And no crate depends on another
    When plan levels are computed
    Then level 0 contains "alpha", "beta", and "gamma"

  Scenario: Linear dependency chain creates one crate per level
    Given a workspace with "base", "middle", and "top"
    And "middle" depends on "base"
    And "top" depends on "middle"
    When plan levels are computed
    Then level 0 contains "base"
    And level 1 contains "middle"
    And level 2 contains "top"

  Scenario: Single crate workspace produces one level
    Given a workspace with a single crate "solo"
    When plan levels are computed
    Then level 0 contains "solo"
    And there is only one level

  Scenario: Diamond dependency creates three levels
    Given a workspace with "root", "left", "right", and "leaf"
    And "left" and "right" depend on "root"
    And "leaf" depends on both "left" and "right"
    When plan levels are computed
    Then level 0 contains "root"
    And level 1 contains "left" and "right"
    And level 2 contains "leaf"

  Scenario: Parallel max_concurrent limits concurrency within a level
    Given a workspace with 10 independent crates
    And parallel max_concurrent is set to 3
    When plan levels are computed and execution begins
    Then at most 3 crates are published concurrently
