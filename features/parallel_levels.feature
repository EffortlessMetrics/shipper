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
